#![doc = include_str!("../README.md")]

use async_trait::async_trait;
use libsql::params;
use time::OffsetDateTime;
use tower_sessions_core::{
    session::{Id, Record},
    session_store::{self, ExpiredDeletion},
    SessionStore,
};

/// An error type for libSQL stores.
#[derive(thiserror::Error, Debug)]
pub enum LibsqlStoreError {
    /// A variant to map `libsql` errors.
    #[error(transparent)]
    Libsql(#[from] libsql::Error),

    /// A variant to map `rmp_serde` encode errors.
    #[error(transparent)]
    Encode(#[from] rmp_serde::encode::Error),

    /// A variant to map `rmp_serde` decode errors.
    #[error(transparent)]
    Decode(#[from] rmp_serde::decode::Error),
}

impl From<LibsqlStoreError> for session_store::Error {
    fn from(err: LibsqlStoreError) -> Self {
        match err {
            LibsqlStoreError::Libsql(inner) => session_store::Error::Backend(inner.to_string()),
            LibsqlStoreError::Decode(inner) => session_store::Error::Decode(inner.to_string()),
            LibsqlStoreError::Encode(inner) => session_store::Error::Encode(inner.to_string()),
        }
    }
}

/// A libSQL session store.
#[derive(Clone)]
pub struct LibsqlStore {
    connection: libsql::Connection,
    table_name: String,
}

// Need this since connection does not implement Debug
impl std::fmt::Debug for LibsqlStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibsqlStore")
            // Probably want to handle this differently
            .field("connection", &std::any::type_name::<libsql::Connection>())
            .field("table_name", &self.table_name)
            .finish()
    }
}

impl LibsqlStore {
    /// Create a new libSQL store with the provided connection pool.
    pub fn new(client: libsql::Connection) -> Self {
        Self {
            connection: client,
            table_name: "tower_sessions".into(),
        }
    }

    /// Set the session table name with the provided name.
    pub fn with_table_name(mut self, table_name: impl AsRef<str>) -> Result<Self, String> {
        let table_name = table_name.as_ref();
        if !is_valid_table_name(table_name) {
            return Err(format!(
                "Invalid table name '{}'. Table names must be alphanumeric and may contain \
                 hyphens or underscores.",
                table_name
            ));
        }

        table_name.clone_into(&mut self.table_name);
        Ok(self)
    }

    /// Migrate the session schema.
    pub async fn migrate(&self) -> libsql::Result<()> {
        let query = format!(
            r#"
            create table if not exists {}
            (
                id text primary key not null,
                data blob not null,
                expiry_date integer not null
            )
            "#,
            self.table_name
        );
        self.connection.execute(&query, ()).await?;

        Ok(())
    }

    /// Checks exitence of the ID. Helps ensure unique values in edge cases.
    async fn id_exists(&self, conn: &libsql::Connection, id: &Id) -> session_store::Result<bool> {
        let query = format!(
            r#"
            select exists(select 1 from {table_name} where id = ?)
            "#,
            table_name = self.table_name
        );

        let res = conn
            .query(&query, params![id.to_string()])
            .await
            .map_err(LibsqlStoreError::Libsql)
            .unwrap()
            .next()
            .await
            .unwrap()
            .unwrap()
            .get_value(0)
            .unwrap();

        Ok(res == libsql::Value::Integer(1))
    }

    /// Save results to DB
    async fn save_with_conn(
        &self,
        conn: &libsql::Connection,
        record: &Record,
    ) -> session_store::Result<()> {
        let query = format!(
            r#"
            insert into {}
              (id, data, expiry_date) values (?, ?, ?)
            on conflict(id) do update set
              data = excluded.data,
              expiry_date = excluded.expiry_date
            "#,
            self.table_name
        );
        conn.execute(
            &query,
            params![
                record.id.to_string(),
                rmp_serde::to_vec(record).map_err(LibsqlStoreError::Encode)?,
                record.expiry_date.unix_timestamp()
            ],
        )
        .await
        .map_err(LibsqlStoreError::Libsql)?;

        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for LibsqlStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        let query = format!(
            r#"
            delete from {table_name}
            where expiry_date < unixepoch('now')
            "#,
            table_name = self.table_name
        );
        self.connection
            .execute(&query, ())
            .await
            .map_err(LibsqlStoreError::Libsql)?;
        Ok(())
    }
}

#[async_trait]
impl SessionStore for LibsqlStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        while self.id_exists(&self.connection, &record.id).await? {
            record.id = Id::default() // Generate a new id
        }

        let conn = self.connection.clone();
        self.save_with_conn(&conn, record).await?;

        Ok(())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let conn = self.connection.clone();
        self.save_with_conn(&conn, record).await
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let query = format!(
            r#"
            select data from {}
            where id = ? and expiry_date > ?
            "#,
            self.table_name
        );

        let mut data = self
            .connection
            .query(
                &query,
                params![
                    session_id.to_string(),
                    OffsetDateTime::now_utc().unix_timestamp()
                ],
            )
            .await
            .map_err(LibsqlStoreError::Libsql)?;

        if let Ok(Some(data)) = data.next().await {
            Ok(Some(
                rmp_serde::from_slice(
                    data.get_value(0)
                        .map_err(LibsqlStoreError::Libsql)
                        .unwrap()
                        .as_blob()
                        .unwrap(),
                )
                .map_err(LibsqlStoreError::Decode)?,
            ))
        } else {
            Ok(None)
        }
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        let query = format!(
            r#"
            delete from {} where id = ?
            "#,
            self.table_name
        );

        self.connection
            .execute(&query, params![session_id.to_string()])
            .await
            .map_err(LibsqlStoreError::Libsql)?;

        Ok(())
    }
}

fn is_valid_table_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod libsql_store_tests {
    use std::collections::HashMap;

    use libsql::Builder;
    use serde_json::Value;
    use tower_sessions::cookie::time::{Duration, OffsetDateTime};

    use super::*;

    #[tokio::test]
    // Quick test to ensure that the db can be connected to, a migration can run,
    // and the table is queried, returning None.
    async fn basic_roundtrip() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        let store = LibsqlStore::new(conn.clone());
        store.migrate().await.unwrap();

        let query = r#"
            select * from tower_sessions limit 1
        "#;

        let row = conn.query(query, ()).await.unwrap().next().await.unwrap();

        assert!(row.is_none());
    }

    #[tokio::test]
    // Test a create with conflict
    async fn create_with_conflict() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        let store = LibsqlStore::new(conn.clone());
        store.migrate().await.unwrap();

        let data: HashMap<String, Value> =
            HashMap::from_iter([("key", "value")].to_vec().iter().map(|(k, v)| {
                (
                    k.to_string(),
                    serde_json::to_value(v).expect("Error encoding"),
                )
            }));

        let mut session_record1 = Record {
            id: Id::default(),
            data,
            expiry_date: OffsetDateTime::now_utc()
                .checked_add(Duration::days(1))
                .expect("Overflow making expiry"),
        };
        store
            .create(&mut session_record1)
            .await
            .expect("Error saving session");

        let mut session_record2 = session_record1.clone();
        store
            .create(&mut session_record2)
            .await
            .expect("Error saving session");

        let loaded1 = store
            .load(&session_record1.id)
            .await
            .expect("Error loading")
            .expect("Value missing");

        let loaded2 = store
            .load(&session_record2.id)
            .await
            .expect("Error loading")
            .expect("Value missing");

        assert_eq!(
            loaded1.data, loaded2.data,
            "Session created with dumplcate data"
        );
        assert_ne!(
            loaded1.id, loaded2.id,
            "Session conflict on id generates a new id"
        );
    }

    #[tokio::test]
    // Test a save and load
    async fn save_and_load() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        let store = LibsqlStore::new(conn.clone());
        store.migrate().await.unwrap();

        let data: HashMap<String, Value> =
            HashMap::from_iter([("key", "value")].to_vec().iter().map(|(k, v)| {
                (
                    k.to_string(),
                    serde_json::to_value(v).expect("Error encoding"),
                )
            }));

        let session_record = Record {
            id: Id::default(),
            data,
            expiry_date: OffsetDateTime::now_utc()
                .checked_add(Duration::days(1))
                .expect("Overflow making expiry"),
        };

        store
            .save(&session_record)
            .await
            .expect("Error saving session");

        let loaded = store
            .load(&session_record.id)
            .await
            .expect("Error loading")
            .expect("Value missing");

        assert_eq!(session_record, loaded, "Save and load match");
    }

    #[tokio::test]
    // Test a delete
    async fn save_and_delete() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        let store = LibsqlStore::new(conn.clone());
        store.migrate().await.unwrap();

        let data: HashMap<String, Value> =
            HashMap::from_iter([("key", "value")].to_vec().iter().map(|(k, v)| {
                (
                    k.to_string(),
                    serde_json::to_value(v).expect("Error encoding"),
                )
            }));

        let session_record = Record {
            id: Id::default(),
            data,
            expiry_date: OffsetDateTime::now_utc()
                .checked_add(Duration::days(1))
                .expect("Overflow making expiry"),
        };

        store
            .save(&session_record)
            .await
            .expect("Error saving session");

        let loaded = store
            .load(&session_record.id)
            .await
            .expect("Error loading")
            .expect("Value missing");

        assert_eq!(session_record, loaded, "Save and load match");

        store
            .delete(&session_record.id)
            .await
            .expect("Error deleting session record");

        let loaded = store.load(&session_record.id).await.expect("Error loading");

        assert!(loaded.is_none())
    }
}
