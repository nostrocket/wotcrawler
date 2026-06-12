//! Shared integration-test fixtures.
//!
//! Every integration test in later plans reuses [`start_postgres`] to obtain an
//! ephemeral Postgres instance (via testcontainers + Docker) and a connection URL.

use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

/// Start an ephemeral Postgres container and return its handle plus a connection URL.
///
/// The caller MUST keep the returned [`ContainerAsync`] alive for the duration of
/// the test — dropping it stops and removes the container.
///
/// Requires a running Docker daemon.
pub async fn start_postgres() -> anyhow::Result<(ContainerAsync<Postgres>, String)> {
    let container = Postgres::default().start().await?;
    let port = container.get_host_port_ipv4(5432).await?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    Ok((container, url))
}
