//! HTTP surface, route for route the same as server.mjs:379-537.

mod benchmarks;
mod health;
mod lookups;
mod operations;
mod path_params;
mod project_diagnostics;
pub mod wire;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::{Method, Uri};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;

use crate::http_errors::route_not_found;
use crate::store::Database;

/// Fastify's default `bodyLimit`.
pub const BODY_LIMIT_BYTES: usize = 1024 * 1024;

pub type SharedDatabase = Arc<Database>;

pub fn build_router(database: SharedDatabase) -> Router {
    Router::new()
        .route("/health", get(health::report_health))
        .route("/v1/operations", post(operations::accept_operation_batch))
        .route(
            "/v1/projects/{project_id}/errors",
            get(project_diagnostics::list_project_errors),
        )
        .route(
            "/v1/projects/{project_id}/context/bootstrap",
            get(project_diagnostics::bootstrap_project_context),
        )
        .route(
            "/v1/projects/{project_id}/benchmarks",
            get(benchmarks::list_programs),
        )
        .route(
            "/v1/projects/{project_id}/benchmarks/{program_id}/evaluation-data",
            get(benchmarks::evaluation_data),
        )
        .route(
            "/v1/projects/{project_id}/benchmarks/{program_id}/report",
            get(benchmarks::latest_report),
        )
        .route(
            "/v1/errors/{error_group_id}",
            get(lookups::diagnose_error_group),
        )
        .route("/v1/prompts/{prompt_id}", get(lookups::show_prompt))
        .route(
            "/v1/projects/{project_id}/analytics",
            get(project_diagnostics::usage_analytics),
        )
        .route("/v1/traces/{trace_id}", get(lookups::trace_events))
        .fallback(unknown_route)
        .method_not_allowed_fallback(unknown_route)
        .layer(DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .with_state(database)
}

async fn unknown_route(method: Method, uri: Uri) -> Response {
    route_not_found(
        method.as_str(),
        uri.path_and_query()
            .map_or(uri.path(), |path| path.as_str()),
    )
}
