//! View specification types — the serde mirror of the JSON schema.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

// ── ViewSpec ──

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewSpec {
    #[serde(rename = "outriderView", default = "one")]
    pub version: u32,
    #[serde(default)]
    pub meta: Meta,
    #[serde(default)]
    pub space: SpaceSpec,
    #[serde(default)]
    pub sets: BTreeMap<String, SetExpr>,
    #[serde(default)]
    pub metrics: BTreeMap<String, ImportedMetric>,
    #[serde(default)]
    pub layers: Vec<LayerSpec>,
    #[serde(default)]
    pub camera: CameraSpec,
}

impl Default for ViewSpec {
    fn default() -> Self {
        ViewSpec {
            version: 1,
            meta: Meta::default(),
            space: SpaceSpec::default(),
            sets: BTreeMap::new(),
            metrics: BTreeMap::new(),
            layers: Vec::new(),
            camera: CameraSpec::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

// ── Space ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceSpec {
    #[serde(default)]
    pub kind: SpaceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regroup: Option<PartitionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<PackOverrides>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SpaceKind {
    #[default]
    Treemap,
    Callgraph,
    Matrix,
}

impl Default for SpaceSpec {
    fn default() -> Self {
        SpaceSpec {
            kind: SpaceKind::default(),
            regroup: None,
            exclude: None,
            pack: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartitionRef {
    pub partition: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_display_lines: Option<u64>,
}

// ── SetRef ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SetRef {
    Name(String),
    Inline(Box<SetExpr>),
}

// ── SetExpr ──
// Full enum so documents validate. Milestone-0 only resolves: Ids, Neighbors, Union, Ref.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetExpr {
    /// Named set reference: {"ref": "hidden"}
    Ref(String),
    /// Explicit IDs: {"ids": ["fn:src/a.rs::foo", "$focus"]}
    Ids(Vec<String>),
    /// Glob over paths: {"glob": "src/auth/**"}
    Glob(String),
    /// By symbol kind: {"kind": "fn"}
    Kind(String),
    /// Union of sets: {"union": [...]}
    Union(Vec<SetExpr>),
    /// Intersection: {"intersect": [...]}
    Intersect(Vec<SetExpr>),
    /// Difference: {"diff": [base, remove]}
    Diff(Box<[SetExpr; 2]>),
    /// Complement: {"not": expr}
    Not(Box<SetExpr>),
    /// Ancestors: {"ancestors": expr}
    Ancestors(Box<SetRef>),
    /// Descendants: {"descendants": expr}
    Descendants(Box<SetRef>),
    /// Children: {"children": expr}
    Children(Box<SetRef>),
    /// File containing symbols: {"fileOf": expr}
    FileOf(Box<SetRef>),
    /// Arrow-key neighbors: {"neighbors": "focus"}
    Neighbors(String),
    /// Fuzzy search: {"fuzzy": "query"}
    Fuzzy(String),
    /// Visible on screen: {"visible": true}
    Visible(bool),
    /// Changed in git: {"changed": "HEAD~1"}
    Changed(String),
    /// Filter by metric: {"where": {"metric": "churn", "op": ">", "value": "p90"}}
    Where(WhereExpr),
    /// Graph reachability: {"reach": {"from": {...}, "relation": "calls", "direction": "out", "depth": 2}}
    Reach(ReachExpr),
    /// Partition community: {"community": {"partition": "communities", "id": "3"}}
    Community(PartitionMember),
    /// Partition layer: {"layer": {"partition": "layers", "id": "domain"}}
    Layer(PartitionMember),
}

// ── Where ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhereExpr {
    pub metric: String,
    pub op: CmpOp,
    pub value: WhereValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum CmpOp {
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WhereValue {
    Percentile(String),
    Number(f64),
}

impl std::cmp::Eq for WhereValue {}

impl Hash for WhereValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            WhereValue::Percentile(s) => {
                0u8.hash(state);
                s.hash(state);
            }
            WhereValue::Number(f) => {
                1u8.hash(state);
                f.to_bits().hash(state);
            }
        }
    }
}

// ── Reach ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachExpr {
    pub from: Box<SetExpr>,
    pub relation: String,
    #[serde(default)]
    pub direction: ReachDirection,
    #[serde(default)]
    pub depth: Depth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReachDirection {
    #[default]
    Out,
    In,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(untagged)]
pub enum Depth {
    N(u32),
    Inf(InfWord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum InfWord {
    Inf,
}

impl Default for Depth {
    fn default() -> Self {
        Depth::N(1)
    }
}

// ── Partition member ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartitionMember {
    pub partition: String,
    #[serde(alias = "name")]
    pub id: String,
}

// ── Layer specs ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LayerSpec {
    Fill(FillSpec),
    Mask(MaskSpec),
    Edges(EdgesSpec),
    Marks(MarksSpec),
    Notes(NotesSpec),
    Panel(PanelSpec),
}

// ── Fill ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FillSpec {
    pub metric: String,
    #[serde(default)]
    pub channel: FillChannel,
    #[serde(default)]
    pub scale: Scale,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp: Option<String>,
}

impl Default for FillSpec {
    fn default() -> Self {
        FillSpec {
            metric: String::new(),
            channel: FillChannel::default(),
            scale: Scale::default(),
            domain: None,
            ramp: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FillChannel {
    #[default]
    Fill,
    Stripe,
    Opacity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scale {
    Linear,
    Log,
    Percentile,
    Threshold(Vec<f64>),
    Categorical,
}

impl Default for Scale {
    fn default() -> Self {
        Scale::Percentile
    }
}

// ── ImportedMetric ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedMetric {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default)]
    pub values: BTreeMap<String, f64>,
}

// ── Mask ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaskSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim_except: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<SetRef>,
    #[serde(default = "default_mask_strength")]
    pub strength: f32,
}

fn default_mask_strength() -> f32 {
    0.7
}

// ── Edges ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgesSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairs: Option<Vec<EdgePair>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_to: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_boundary: Option<Boundary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    #[serde(default)]
    pub min_weight: f64,
    #[serde(default)]
    pub style: EdgeStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl Default for EdgesSpec {
    fn default() -> Self {
        EdgesSpec {
            relation: Some("calls".into()),
            pairs: None,
            within: None,
            incident_to: None,
            cross_boundary: None,
            direction: None,
            min_weight: 0.0,
            style: EdgeStyle::default(),
            color: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgePair {
    pub from: String,
    pub to: String,
    #[serde(default = "one_f64")]
    pub weight: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
}

fn one_f64() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Boundary {
    Folder,
    Partition(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Up,
    Down,
    #[default]
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum EdgeStyle {
    #[default]
    Solid,
    Dashed,
}

pub enum EdgeSource<'a> {
    Relation(&'a str),
    Pairs(&'a [EdgePair]),
}

impl EdgesSpec {
    pub fn source(&self) -> Result<EdgeSource<'_>, &'static str> {
        match (&self.relation, &self.pairs) {
            (Some(r), None) => Ok(EdgeSource::Relation(r)),
            (None, Some(p)) => Ok(EdgeSource::Pairs(p)),
            _ => Err("edges.source"),
        }
    }
}

// ── Marks ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarksSpec {
    pub on: MarkTarget,
    pub kind: MarkKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MarkTarget {
    Set(SetRef),
    // Anchors variant for later
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MarkKind {
    FocusRing,
    Neighbor,
    Selection,
    Hotspot,
    Cycle,
    LayeringViolation,
    AgentFlag,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkStyle {
    Nav,
    Structural,
    Agent,
}

// ── Notes ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct NotesSpec(pub Vec<NoteSpec>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NoteSpec {
    pub at: NoteAnchor,
    pub source: NoteSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<(f32, f32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readout: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NoteAnchor {
    Live(LiveAnchor),
    Symbol(String),
    Range {
        symbol: String,
        #[serde(flatten)]
        span: AnchorSpan,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveAnchor {
    #[serde(rename = "$focus")]
    Focus,
    #[serde(rename = "$hover")]
    Hover,
    #[serde(rename = "$selection")]
    Selection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum AnchorSpan {
    Range([usize; 2]),
    Lines([usize; 2]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoteSource {
    Doc,
    Metric,
    Agent,
}

// ── Panel ──

/// Newtype for metric references (e.g. "churn", "coverage").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MetricRef(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PanelSpec {
    pub rows: PanelRows,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<MetricRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_by: Option<SortKey>,
    #[serde(default)]
    pub dock: Dock,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl Default for PanelSpec {
    fn default() -> Self {
        PanelSpec {
            rows: PanelRows::Set(SetRef::Inline(Box::new(SetExpr::Ids(vec![])))),
            columns: vec![],
            sort_by: None,
            dock: Dock::Float,
            title: None,
            id: None,
            limit: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum PanelRows {
    Set(SetRef),
    EdgeGroups { of: SetRef, relation: String, direction: EdgeDirection },
    Matrix { space: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EdgeDirection { In, Out }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Dock { Left, Right, Bottom, #[default] Float }

/// Sort key: either a builtin sort or a metric reference.
/// Serde: try Builtin first since MetricRef accepts any string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SortKey {
    Builtin(BuiltinSort),
    Metric(MetricRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuiltinSort { Name, NameLength }

// ── Camera ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(default)]
    pub follow: FollowMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FollowMode {
    #[default]
    Focus,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    #[serde(flatten)]
    pub target: StepTarget,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub push: Vec<LayerSpec>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pop: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepTarget {
    Frame(SetRef),
    Focus(String),
    Home(HomeFlag),
}

/// Unit marker that (de)serializes as the boolean `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HomeFlag;

impl Serialize for HomeFlag {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for HomeFlag {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = bool::deserialize(deserializer)?;
        if v {
            Ok(HomeFlag)
        } else {
            Err(serde::de::Error::custom("home must be true"))
        }
    }
}
