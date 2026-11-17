//! MCP server for the `nml-viewer` (stdio): starts the viewer with a control channel or attaches
//! to a running one and exposes input, clock, state, screenshot and window as tools
//! (`docs/viewer-control.md`).

mod connection;
mod launch;
mod session;
mod tools;
mod tools_ki;
mod tools_script;
mod tools_ui;

use rmcp::ServiceExt;

use crate::launch::repo_root;
use crate::session::Session;
use crate::tools::ViewerServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout belongs to the MCP protocol; this server writes nothing itself.
    let exe = std::env::current_exe()?;
    let repo = repo_root(std::env::var_os("NML_REPO"), &exe);
    let service = ViewerServer::new(Session::new(repo))
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
