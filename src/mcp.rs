//! Caret MCP Server — Model Context Protocol over JSON-RPC / HTTP
//!
//! Exposes local datasets as **Tools** and **Resources** to any MCP-compatible
//! LLM client (Claude Desktop, Cursor, etc.).  Runs on a background Tokio task
//! so the TUI never blocks.
//!
//! # Protocol
//!
//! MCP uses JSON-RPC 2.0 over HTTP (or stdio).  We implement the minimal
//! server surface:
//!
//! | Method                        | Purpose                                    |
//! |-------------------------------|--------------------------------------------|
//! | `initialize`                  | Handshake — returns server capabilities    |
//! | `tools/list`                  | Enumerate available tools                  |
//! | `tools/call`                  | Execute a tool (e.g. `search_dataset`)     |
//! | `resources/list`              | Enumerate exposed dataset resources        |
//! | `resources/read`              | Read a specific resource (line range)      |
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────┐   JSON-RPC   ┌──────────────┐    zero-copy   ┌──────────┐
//! │  LLM Client  │─────────────▶│  Axum Router │───────────────▶│  Dataset │
//! │ (Claude/etc) │◀─────────────│  (async)     │◀───────────────│  (mmap)  │
//! └──────────────┘              └──────────────┘                └──────────┘
//! ```

use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::data::Dataset;
use crate::engine::{DedupEngine, DedupStrategy};

// ─── JSON-RPC 2.0 types ────────────────────────────────────────────────────

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<serde_json::Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }
}

// ─── MCP protocol types ────────────────────────────────────────────────────

/// MCP server capabilities announced during `initialize`.
#[derive(Debug, Serialize)]
struct ServerCapabilities {
    tools: ToolsCapability,
    resources: ResourcesCapability,
}

#[derive(Debug, Serialize)]
struct ToolsCapability {
    #[serde(rename = "listChanged")]
    list_changed: bool,
}

#[derive(Debug, Serialize)]
struct ResourcesCapability {
    #[serde(rename = "listChanged")]
    list_changed: bool,
}

#[derive(Debug, Serialize)]
struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    server_info: ServerInfo,
}

#[derive(Debug, Serialize)]
struct ServerInfo {
    name: String,
    version: String,
}

/// MCP tool descriptor.
#[derive(Debug, Serialize)]
struct ToolDescriptor {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

/// MCP resource descriptor.
#[derive(Debug, Serialize)]
struct ResourceDescriptor {
    uri: String,
    name: String,
    description: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
}

/// Result content block returned by tool calls.
#[derive(Debug, Serialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

// ─── Shared state ───────────────────────────────────────────────────────────

/// State shared between the Axum handler and the dataset.
/// `RwLock` allows concurrent reads from multiple MCP clients without
/// blocking the TUI thread.
pub struct McpState {
    pub dataset: Arc<Dataset>,
    pub dataset_path: String,
}

pub type SharedMcpState = Arc<RwLock<McpState>>;

// ─── Server bootstrap ──────────────────────────────────────────────────────

/// Start the MCP server on the given port.
///
/// Returns a `JoinHandle` — the caller can `.abort()` it for clean shutdown.
/// Designed to be spawned from `main()` via `tokio::spawn`, ensuring the TUI
/// event loop remains unblocked.
pub async fn start_mcp_server(
    dataset: Arc<Dataset>,
    dataset_path: String,
    port: u16,
) -> Result<()> {
    let state: SharedMcpState = Arc::new(RwLock::new(McpState {
        dataset,
        dataset_path,
    }));

    let app = Router::new()
        .route("/", post(handle_jsonrpc))
        .route("/health", get(health_check))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    info!("MCP server listening on http://127.0.0.1:{}", port);

    axum::serve(listener, app).await?;

    Ok(())
}

/// Health-check endpoint (useful for readiness probes).
async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "ok", "server": "caret-mcp"})))
}

// ─── JSON-RPC dispatcher ───────────────────────────────────────────────────

async fn handle_jsonrpc(
    State(state): State<SharedMcpState>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let response = match req.method.as_str() {
        "initialize" => handle_initialize(req.id),
        "initialized" => {
            // Notification — no response required, but we reply with empty success
            JsonRpcResponse::success(req.id, serde_json::json!({}))
        }
        "tools/list" => handle_tools_list(req.id),
        "tools/call" => handle_tools_call(req.id, req.params, &state).await,
        "resources/list" => handle_resources_list(req.id, &state).await,
        "resources/read" => handle_resources_read(req.id, req.params, &state).await,
        _ => JsonRpcResponse::error(
            req.id,
            -32601,
            format!("Method not found: {}", req.method),
        ),
    };

    Json(response)
}

// ─── Method handlers ────────────────────────────────────────────────────────

fn handle_initialize(id: Option<serde_json::Value>) -> JsonRpcResponse {
    let result = InitializeResult {
        protocol_version: "2024-11-05".into(),
        capabilities: ServerCapabilities {
            tools: ToolsCapability {
                list_changed: false,
            },
            resources: ResourcesCapability {
                list_changed: false,
            },
        },
        server_info: ServerInfo {
            name: "caret".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    };

    JsonRpcResponse::success(id, serde_json::to_value(result).expect("InitializeResult is serializable"))
}

fn handle_tools_list(id: Option<serde_json::Value>) -> JsonRpcResponse {
    let tools = vec![
        ToolDescriptor {
            name: "search_dataset".into(),
            description: "Search the loaded dataset using regex pattern matching. \
                          Returns matching lines with line numbers. Uses Caret's \
                          SIMD-optimized engine for high-throughput scanning."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Regex pattern to search for in the dataset"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 50)",
                        "default": 50
                    },
                    "context_lines": {
                        "type": "integer",
                        "description": "Number of surrounding lines to include (default: 0)",
                        "default": 0
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDescriptor {
            name: "dataset_info".into(),
            description: "Get metadata about the currently loaded dataset: line count, \
                          file size, format, and deduplication statistics."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDescriptor {
            name: "get_lines".into(),
            description: "Retrieve specific lines from the dataset by index range. \
                          Supports O(1) random access via memory-mapped byte offsets."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "start": {
                        "type": "integer",
                        "description": "Start line index (0-based)"
                    },
                    "count": {
                        "type": "integer",
                        "description": "Number of lines to retrieve (default: 10, max: 500)",
                        "default": 10
                    }
                },
                "required": ["start"]
            }),
        },
        ToolDescriptor {
            name: "dedup_scan".into(),
            description: "Run SIMD-accelerated near-duplicate detection on the dataset. \
                          Returns duplicate statistics and sample duplicate pairs."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "strategy": {
                        "type": "string",
                        "enum": ["exact", "simhash"],
                        "description": "Dedup strategy (default: simhash)",
                        "default": "simhash"
                    },
                    "threshold": {
                        "type": "integer",
                        "description": "SimHash Hamming distance threshold (default: 3)",
                        "default": 3
                    }
                }
            }),
        },
    ];

    JsonRpcResponse::success(
        id,
        serde_json::json!({ "tools": serde_json::to_value(&tools).expect("ToolDescriptor is serializable") }),
    )
}

/// Execute a tool call.
async fn handle_tools_call(
    id: Option<serde_json::Value>,
    params: serde_json::Value,
    state: &SharedMcpState,
) -> JsonRpcResponse {
    let tool_name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    match tool_name {
        "search_dataset" => tool_search_dataset(id, arguments, state).await,
        "dataset_info" => tool_dataset_info(id, state).await,
        "get_lines" => tool_get_lines(id, arguments, state).await,
        "dedup_scan" => tool_dedup_scan(id, arguments, state).await,
        _ => JsonRpcResponse::error(
            id,
            -32602,
            format!("Unknown tool: {}", tool_name),
        ),
    }
}

/// `search_dataset` — regex search over the mmap'd dataset.
///
/// Uses `regex::Regex` (which auto-selects SIMD acceleration on x86_64)
/// to scan every line.  Runs on a `spawn_blocking` thread so the async
/// runtime stays responsive.
async fn tool_search_dataset(
    id: Option<serde_json::Value>,
    args: serde_json::Value,
    state: &SharedMcpState,
) -> JsonRpcResponse {
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) => q.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                -32602,
                "Missing required parameter: query".into(),
            )
        }
    };

    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(50) as usize;

    let context_lines = args
        .get("context_lines")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let state_guard = state.read().await;
    let dataset = Arc::clone(&state_guard.dataset);
    let total_lines = state_guard.dataset.line_count();
    drop(state_guard);
    let query_clone = query.clone();

    // Offload CPU-intensive regex scan to blocking threadpool
    let result = tokio::task::spawn_blocking(move || {
        search_dataset_impl(&dataset, &query_clone, max_results, context_lines)
    })
    .await;

    match result {
        Ok(Ok(matches)) => {
            let text = if matches.is_empty() {
                format!("No matches found for pattern: `{}`", query)
            } else {
                let header = format!(
                    "Found {} match(es) for `{}` in {} lines:\n\n",
                    matches.len(),
                    query,
                    total_lines,
                );
                let body: Vec<String> = matches
                    .iter()
                    .map(|(line_num, content)| format!("L{}: {}", line_num + 1, content))
                    .collect();
                format!("{}{}", header, body.join("\n"))
            };

            let content = vec![ContentBlock {
                content_type: "text".into(),
                text,
            }];

            JsonRpcResponse::success(
                id,
                serde_json::json!({ "content": serde_json::to_value(&content).expect("ContentBlock is serializable") }),
            )
        }
        Ok(Err(e)) => JsonRpcResponse::error(id, -32603, format!("Search error: {}", e)),
        Err(e) => JsonRpcResponse::error(id, -32603, format!("Task join error: {}", e)),
    }
}

/// CPU-bound regex search — runs on the blocking threadpool.
fn search_dataset_impl(
    dataset: &Dataset,
    pattern: &str,
    max_results: usize,
    context_lines: usize,
) -> Result<Vec<(usize, String)>> {
    let re = regex::Regex::new(pattern)?;
    let mut matches = Vec::new();
    let line_count = dataset.line_count();

    for i in 0..line_count {
        if matches.len() >= max_results {
            break;
        }

        if let Some(line) = dataset.get_line(i) {
            if re.is_match(line) {
                if context_lines > 0 {
                    // Include surrounding context
                    let start = i.saturating_sub(context_lines);
                    let end = (i + context_lines + 1).min(line_count);
                    let mut ctx = String::new();
                    for j in start..end {
                        if let Some(ctx_line) = dataset.get_line(j) {
                            let marker = if j == i { ">>>" } else { "   " };
                            ctx.push_str(&format!("{} L{}: {}\n", marker, j + 1, ctx_line));
                        }
                    }
                    matches.push((i, ctx));
                } else {
                    matches.push((i, line.to_string()));
                }
            }
        }
    }

    Ok(matches)
}

/// `dataset_info` — return metadata about the loaded dataset.
async fn tool_dataset_info(
    id: Option<serde_json::Value>,
    state: &SharedMcpState,
) -> JsonRpcResponse {
    let state = state.read().await;
    let ds = &state.dataset;

    let info = serde_json::json!({
        "path": state.dataset_path,
        "format": ds.format_name(),
        "line_count": ds.line_count(),
        "size_bytes": ds.size,
        "size_human": ds.size_human(),
    });

    let text = format!(
        "Dataset: {}\nFormat: {}\nLines: {}\nSize: {} ({} bytes)",
        state.dataset_path,
        ds.format_name(),
        ds.line_count(),
        ds.size_human(),
        ds.size,
    );

    let content = vec![ContentBlock {
        content_type: "text".into(),
        text,
    }];

    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "content": serde_json::to_value(&content).expect("ContentBlock is serializable"),
            "metadata": info,
        }),
    )
}

/// `get_lines` — O(1) random access to specific line ranges.
async fn tool_get_lines(
    id: Option<serde_json::Value>,
    args: serde_json::Value,
    state: &SharedMcpState,
) -> JsonRpcResponse {
    let start = args
        .get("start")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let count = args
        .get("count")
        .and_then(|v| v.as_u64())
        .unwrap_or(10) as usize;
    let count = count.min(500); // Safety cap

    let state = state.read().await;
    let ds = &state.dataset;

    let mut lines = Vec::new();
    for i in start..(start + count).min(ds.line_count()) {
        if let Some(line) = ds.get_line(i) {
            lines.push(format!("L{}: {}", i + 1, line));
        }
    }

    let text = if lines.is_empty() {
        format!("No lines found at index {} (dataset has {} lines)", start, ds.line_count())
    } else {
        format!(
            "Lines {}-{} of {}:\n\n{}",
            start + 1,
            (start + count).min(ds.line_count()),
            ds.line_count(),
            lines.join("\n")
        )
    };

    let content = vec![ContentBlock {
        content_type: "text".into(),
        text,
    }];

    JsonRpcResponse::success(
        id,
        serde_json::json!({ "content": serde_json::to_value(&content).expect("ContentBlock is serializable") }),
    )
}

/// `dedup_scan` — run the SIMD dedup engine and return results.
async fn tool_dedup_scan(
    id: Option<serde_json::Value>,
    args: serde_json::Value,
    state: &SharedMcpState,
) -> JsonRpcResponse {
    let strategy_str = args
        .get("strategy")
        .and_then(|v| v.as_str())
        .unwrap_or("simhash");
    let threshold = args
        .get("threshold")
        .and_then(|v| v.as_u64())
        .unwrap_or(3) as u32;

    let strategy = match strategy_str {
        "exact" => DedupStrategy::Exact,
        _ => DedupStrategy::SimHash { threshold },
    };

    let state = state.read().await;
    let dataset = Arc::clone(&state.dataset);

    let result = tokio::task::spawn_blocking(move || {
        let engine = DedupEngine::new(strategy);
        engine.scan(&dataset)
    })
    .await;

    match result {
        Ok(dr) => {
            // Collect a few sample duplicate pairs
            let mut sample_pairs: Vec<serde_json::Value> = Vec::new();
            let line_count = dr.total_lines;
            for i in 0..line_count {
                if sample_pairs.len() >= 5 {
                    break;
                }
                if dr.is_duplicate(i) {
                    let canonical = dr.canonical_map[i];
                    sample_pairs.push(serde_json::json!({
                        "duplicate_line": i + 1,
                        "original_line": canonical + 1,
                        "hamming_distance": dr.fingerprints[i].hamming_distance(dr.fingerprints[canonical]),
                    }));
                }
            }

            let text = format!(
                "Dedup Scan Results (strategy: {}):\n\
                 Total lines: {}\n\
                 Unique: {}\n\
                 Duplicates: {} ({:.1}%)\n\
                 Scan time: {:.1}ms",
                dr.strategy,
                dr.total_lines,
                dr.unique_count,
                dr.duplicate_count,
                dr.dedup_ratio() * 100.0,
                dr.elapsed_us as f64 / 1000.0,
            );

            let content = vec![ContentBlock {
                content_type: "text".into(),
                text,
            }];

            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "content": serde_json::to_value(&content).expect("ContentBlock is serializable"),
                    "metadata": {
                        "total_lines": dr.total_lines,
                        "unique_count": dr.unique_count,
                        "duplicate_count": dr.duplicate_count,
                        "dedup_ratio": dr.dedup_ratio(),
                        "elapsed_ms": dr.elapsed_us as f64 / 1000.0,
                        "sample_pairs": sample_pairs,
                    }
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, -32603, format!("Dedup scan failed: {}", e)),
    }
}

/// Handle `resources/list` — expose the loaded dataset as a resource.
async fn handle_resources_list(
    id: Option<serde_json::Value>,
    state: &SharedMcpState,
) -> JsonRpcResponse {
    let state = state.read().await;

    let resources = vec![ResourceDescriptor {
        uri: format!("caret://dataset/{}", state.dataset_path),
        name: format!("Dataset: {}", state.dataset_path),
        description: format!(
            "{} file with {} lines ({})",
            state.dataset.format_name(),
            state.dataset.line_count(),
            state.dataset.size_human(),
        ),
        mime_type: "application/jsonl".into(),
    }];

    JsonRpcResponse::success(
        id,
        serde_json::json!({ "resources": serde_json::to_value(&resources).expect("ResourceDescriptor is serializable") }),
    )
}

/// Handle `resources/read` — return a slice of the dataset.
async fn handle_resources_read(
    id: Option<serde_json::Value>,
    params: serde_json::Value,
    state: &SharedMcpState,
) -> JsonRpcResponse {
    let _uri = params
        .get("uri")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let state = state.read().await;
    let ds = &state.dataset;

    // Return first 100 lines as a preview
    let preview_count = 100.min(ds.line_count());
    let mut lines = Vec::with_capacity(preview_count);
    for i in 0..preview_count {
        if let Some(line) = ds.get_line(i) {
            lines.push(line.to_string());
        }
    }

    let text = lines.join("\n");

    let contents = vec![serde_json::json!({
        "uri": format!("caret://dataset/{}", state.dataset_path),
        "mimeType": "application/jsonl",
        "text": text,
    })];

    JsonRpcResponse::success(
        id,
        serde_json::json!({ "contents": contents }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jsonrpc_response_success() {
        let resp = JsonRpcResponse::success(Some(serde_json::json!(1)), serde_json::json!({"ok": true}));
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[test]
    fn test_jsonrpc_response_error() {
        let resp = JsonRpcResponse::error(Some(serde_json::json!(1)), -32600, "Invalid request".into());
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }
}
