//! TUI Command Channel — enables MCP tools to control the TUI
//!
//! The MCP server sends commands through an mpsc channel, which the TUI
//! event loop polls to react to AI-driven navigation requests.

use tokio::sync::mpsc;

/// Commands that can be sent from MCP to the TUI
#[derive(Debug, Clone)]
pub enum TuiCommand {
    /// Jump to a specific line number (0-indexed)
    JumpToLine(usize),
    
    /// Toggle view mode: Text → TokenXray → Tree → Text
    ToggleView,
    
    /// Set view mode directly
    SetViewMode(ViewModeCmd),
    
    /// Show or hide the detail panel
    ShowDetail(bool),
    
    /// Scroll down by N lines
    ScrollDown(usize),
    
    /// Scroll up by N lines
    ScrollUp(usize),
    
    /// Jump to top of dataset
    GotoTop,
    
    /// Jump to bottom of dataset
    GotoBottom,
}

/// View mode variants for SetViewMode command
#[derive(Debug, Clone, Copy)]
pub enum ViewModeCmd {
    Text,
    TokenXray,
    Tree,
}

/// Type alias for the command sender (used by MCP)
pub type TuiCommandSender = mpsc::UnboundedSender<TuiCommand>;

/// Type alias for the command receiver (used by TUI loop)
pub type TuiCommandReceiver = mpsc::UnboundedReceiver<TuiCommand>;

/// Create a new command channel pair
pub fn command_channel() -> (TuiCommandSender, TuiCommandReceiver) {
    mpsc::unbounded_channel()
}
