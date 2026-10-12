//! Domain queries and the application's combined domain/library migrations.
pub mod generated;
#[cfg(test)]
mod tests;

pub static MIGRATOR: std::sync::LazyLock<sqlx::migrate::Migrator> =
    std::sync::LazyLock::new(|| {
        sqlx::migrate::Migrator::with_migrations(
            sqlx::migrate!()
                .iter()
                .chain(apalis_postgres::PostgresStorage::migrations().iter())
                .cloned()
                .collect(),
        )
    });

/// Preserve optional lookup semantics without swallowing database failures.
pub fn optional<T>(result: Result<T, sqlx::Error>) -> Result<Option<T>, sqlx::Error> {
    match result {
        Ok(row) => Ok(Some(row)),
        Err(sqlx::Error::RowNotFound) => Ok(None),
        Err(error) => Err(error),
    }
}
