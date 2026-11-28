use std::sync::Arc;

use axum::http::request::Parts;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
    InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use tokio::net::TcpListener;

use crate::common::error::AppError;
use crate::common::http::HttpDiscordApi;
use crate::output::Payload;

use super::request_headers::{RequestHeaders, parse_request_headers};
use super::tools::{call_tool, tool_specs};

pub(crate) const MCP_PATH: &str = "/mcp";

/// Stateless MCP handler: it keeps no bot token and no session. Each request
/// brings its own `Authorization: Bot <token>` and channel headers, so one
/// server can serve several bots side by side.
#[derive(Clone)]
struct DiscordMcpServer {
    client: reqwest::Client,
    discord_base_url: Arc<str>,
}

impl ServerHandler for DiscordMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("discord", env!("CARGO_PKG_VERSION")))
    }

    // Headers are checked at initialize too, so a misconfigured app fails when
    // it is added, not on its first tool call.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        request_headers(&context)?;
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        request_headers(&context)?;
        let tools = tool_specs()
            .into_iter()
            .map(|spec| {
                let serde_json::Value::Object(schema) = spec.input_schema else {
                    unreachable!("tool_specs builds every input schema as a JSON object");
                };
                Tool::new(spec.name, spec.description, schema)
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let headers = request_headers(&context)?;
        let api = HttpDiscordApi::with_client(
            self.client.clone(),
            headers.token.clone(),
            &self.discord_base_url,
        );
        let arguments = request.arguments.unwrap_or_default();
        let result = call_tool(&api, &headers, &request.name, arguments).await;
        eprintln!("{}", call_log_line(&request.name, &result));
        Ok(tool_result(&result).into())
    }
}

/// Header problems are protocol errors: the request cannot be routed to any
/// bot, so no tool runs.
fn request_headers(context: &RequestContext<RoleServer>) -> Result<RequestHeaders, McpError> {
    let parts = context.extensions.get::<Parts>().ok_or_else(|| {
        McpError::internal_error("HTTP request parts missing from the MCP context", None)
    })?;
    parse_request_headers(&parts.headers)
        .map_err(|err| McpError::invalid_request(err.message, None))
}

/// The tool result carries the same JSON as the CLI's `--json` envelope:
/// `data` on success, `error` (`kind`, `message`, …) on failure.
fn tool_result(result: &Result<Payload, AppError>) -> CallToolResult {
    match result {
        Ok(payload) => CallToolResult::structured(to_json(payload)),
        Err(err) => CallToolResult::structured_error(to_json(err)),
    }
}

fn to_json(value: &impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(value).expect("payload and error types serialize as plain data")
}

/// One stderr line per tool call. It is built from the tool name and the
/// outcome only — the request headers (and so the token) never reach it.
fn call_log_line(tool: &str, result: &Result<Payload, AppError>) -> String {
    match result {
        Ok(_) => format!("mcp: {tool} ok"),
        Err(err) => format!("mcp: {tool} failed: {}", err.to_human()),
    }
}

/// Serves MCP Streamable HTTP on `listener` at [`MCP_PATH`] until `shutdown`
/// resolves. `discord_base_url` is Discord's REST base in production.
pub(crate) async fn serve(
    listener: TcpListener,
    discord_base_url: &str,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let handler = DiscordMcpServer {
        client: crate::common::http::build_client(),
        discord_base_url: Arc::from(discord_base_url),
    };
    // Stateless + plain JSON responses: every POST is answered on its own, so
    // there is no session to lose when the server restarts.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true);
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    let app = axum::Router::new().route_service(MCP_PATH, service);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
mod tests;
