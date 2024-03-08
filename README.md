# tower-sessions-libsql-store
A small library for using [tower-sessions](https://github.com/maxcountryman/tower-sessions) with [libsql](https://github.com/tursodatabase/libsql).

libSQL is a fork of SQLite, that is accessible over network requests. Ideally I would love to use [sqlx-store](https://github.com/maxcountryman/tower-sessions-stores/tree/main/sqlx-store), though that is not possible due to a [current lack of support](https://github.com/launchbadge/sqlx/issues/2674).

## ⚠️  Disclaimer
This package is in beta.

## Usage
See [`/examples`](./examples) folder.

Note the `embedded_replica` example requires a live turso database to use for embedded replication. Install the [turso cli](https://github.com/tursodatabase/turso-cli) and run `turso dev`.

## Tests
The tests are copied from [tower-session-stores](https://github.com/maxcountryman/tower-sessions-stores). Run them with `cargo test`.
