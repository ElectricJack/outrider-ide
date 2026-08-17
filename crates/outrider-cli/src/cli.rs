use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "outrider-cli", about = "Control a running outrider instance")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Project root directory (default: search upward for .git or .outrider)
    #[arg(long, global = true)]
    pub project: Option<String>,

    /// Output JSON instead of human-readable text
    #[arg(long, global = true)]
    pub json: bool,

    /// RPC timeout in milliseconds
    #[arg(long, global = true, default_value = "5000")]
    pub timeout: u64,
}

#[derive(Subcommand)]
pub enum Command {
    /// Define a named set
    Set {
        /// Name for the set
        name: String,
        #[command(flatten)]
        args: SetArgs,
    },
    /// Add a mask layer (dim everything except a set, or dim a set)
    Mask {
        /// Dim everything except this set
        #[arg(long)]
        dim_except: Option<String>,
        /// Dim this set
        #[arg(long)]
        dim: Option<String>,
        /// Mask strength (0.0-1.0)
        #[arg(long, default_value = "0.8")]
        strength: f32,
    },
    /// Frame the camera on a set
    Frame {
        /// Set name to frame
        set: String,
    },
    /// Camera: go home (fit entire project)
    Home,
    /// Get the current view spec
    #[command(name = "view")]
    View {
        #[command(subcommand)]
        action: ViewAction,
    },
    /// Manage layers
    Layer {
        #[command(subcommand)]
        action: LayerAction,
    },
    /// Attach an agent note to a symbol
    Note {
        /// Symbol ID, optionally with :startLine-endLine
        symbol: String,
        /// Note text
        text: String,
    },
    /// Open a data panel for a set
    Panel {
        /// Set name whose rows to display
        set: String,
        /// Comma-separated metric columns (e.g. churn,fanin)
        #[arg(long, short = 'c')]
        columns: Option<String>,
        /// Sort key: "name", "nameLength", or a metric name
        #[arg(long, short = 's')]
        sort: Option<String>,
        /// Dock position: left, right, bottom, float (default: float)
        #[arg(long, short = 'd', default_value = "float")]
        dock: String,
        /// Panel title
        #[arg(long, short = 't')]
        title: Option<String>,
        /// Max rows to display
        #[arg(long, short = 'l')]
        limit: Option<usize>,
        /// Stable panel id
        #[arg(long)]
        id: Option<String>,
    },
    /// Color the treemap by a metric
    Fill {
        /// Metric name (e.g. churn, complexity, coverage)
        metric: String,
        /// Fill channel: fill, stripe, or opacity
        #[arg(long, default_value = "fill")]
        channel: String,
        /// Scale: linear, log, percentile, categorical
        #[arg(long, default_value = "percentile")]
        scale: String,
        /// Color ramp name
        #[arg(long)]
        ramp: Option<String>,
        /// Restrict domain to a named set
        #[arg(long)]
        domain: Option<String>,
    },
    /// Import or manage metrics
    Metric {
        #[command(subcommand)]
        action: MetricAction,
    },
    /// Query data from a running instance
    Query {
        #[command(subcommand)]
        action: QueryAction,
    },
    /// Guided tour commands
    Tour {
        #[command(subcommand)]
        action: TourAction,
    },
    /// Camera follow mode
    Camera {
        #[command(subcommand)]
        action: CameraAction,
    },
    /// Check connection to a running instance
    Status,
}

#[derive(Subcommand)]
pub enum MetricAction {
    /// Import metric values from a JSON file
    Import {
        /// Metric name
        name: String,
        /// Path to JSON file with metric values (- for stdin)
        file: String,
    },
}

#[derive(Subcommand)]
pub enum QueryAction {
    /// Query metric readouts for a symbol
    Metrics {
        /// Symbol ID (wire format, e.g. "fn:src/a.rs::foo")
        symbol: String,
    },
}

#[derive(Subcommand)]
pub enum TourAction {
    /// Add a step to the tour
    Add {
        /// Step JSON (e.g. '{"home": true}' or '{"frame": "mySet"}')
        step: String,
    },
    /// Start playing the tour from the beginning
    Play,
    /// Advance to the next step
    Next,
    /// Go back to the previous step
    Prev,
    /// Jump to a specific step by index
    Goto {
        /// Step index (0-based)
        index: usize,
    },
    /// Stop the tour
    Stop,
    /// Replace all steps from a JSON file
    SetSteps {
        /// Path to JSON file with step array (- for stdin)
        file: String,
    },
    /// Load navigation history as tour steps
    LoadHistory,
}

#[derive(Subcommand)]
pub enum CameraAction {
    /// Set follow mode (focus or none)
    Follow {
        /// Follow mode: focus or none
        mode: String,
    },
}

#[derive(Subcommand)]
pub enum ViewAction {
    /// Apply a view spec from a file
    Apply {
        /// Path to view spec JSON file (- for stdin)
        file: String,
    },
    /// Get the current view spec
    Get {
        /// Output file (default: stdout)
        #[arg(short)]
        o: Option<String>,
    },
    /// Clear layers, sets, or all
    Clear {
        #[arg(long)]
        layers: bool,
        #[arg(long)]
        sets: bool,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum LayerAction {
    /// List current layers
    List,
    /// Remove the top layer
    Pop,
    /// Remove a layer by index
    Rm {
        index: usize,
    },
}

#[derive(clap::Args)]
pub struct SetArgs {
    /// Glob pattern (repeatable)
    #[arg(long)]
    pub glob: Vec<String>,
    /// Symbol kind filter
    #[arg(long)]
    pub kind: Vec<String>,
    /// Fuzzy search query
    #[arg(long)]
    pub fuzzy: Option<String>,
    /// Explicit symbol IDs
    #[arg(long)]
    pub ids: Vec<String>,
    /// Reference an existing set
    #[arg(long, name = "from")]
    pub from_set: Option<String>,
    /// Raw SetExpr JSON (mutually exclusive with other flags)
    #[arg(long)]
    pub expr: Option<String>,
}
