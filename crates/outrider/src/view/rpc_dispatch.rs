//! RPC method dispatch — maps JSON-RPC method names to ViewCommands.

use crate::treemap::TreemapView;
use crate::view::rpc::{RpcError, RpcRequest};
use outrider_index::{SymbolId, SymbolNode};
use outrider_view::command::{CameraCommand, TourCommand, ViewCommand};

impl TreemapView {
    /// Dispatch a single RPC request to the appropriate handler.
    pub(crate) fn dispatch_rpc(
        &mut self,
        req: &RpcRequest,
        _window: &gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Result<serde_json::Value, RpcError> {
        match req.method.as_str() {
            "view.apply" => {
                let p: ViewApplyParams = parse_params(&req.params)?;
                self.apply_and_respond(ViewCommand::Apply(p.spec))
            }
            "view.patch" => {
                let p: ViewPatchParams = parse_params(&req.params)?;
                self.apply_and_respond(ViewCommand::Patch(p.patch))
            }
            "view.get" | "query.view" => Ok(serde_json::json!({"spec": self.view_spec})),
            "view.clear" => {
                let p: ViewClearParams = parse_params(&req.params)?;
                let scope = match p.scope.as_str() {
                    "layers" => outrider_view::command::ClearScope::Layers,
                    "sets" => outrider_view::command::ClearScope::Sets,
                    "all" => outrider_view::command::ClearScope::All,
                    other => {
                        return Err(RpcError {
                            code: -32602,
                            message: format!("unknown clear scope: {other}"),
                            data: None,
                        })
                    }
                };
                self.apply_and_respond(ViewCommand::Clear(scope))
            }
            "set.define" => {
                let p: SetDefineParams = parse_params(&req.params)?;
                self.apply_and_respond(ViewCommand::DefineSet {
                    name: p.name,
                    expr: p.expr,
                })
            }
            "layer.push" => {
                let p: LayerPushParams = parse_params(&req.params)?;
                self.apply_and_respond(ViewCommand::PushLayer(p.layer))
            }
            "layer.pop" => self.apply_and_respond(ViewCommand::PopLayer),
            "layer.remove" => {
                let p: LayerRemoveParams = parse_params(&req.params)?;
                self.apply_and_respond(ViewCommand::RemoveLayer(p.index))
            }
            "layer.list" => {
                let summaries: Vec<serde_json::Value> = self
                    .view_spec
                    .layers
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        serde_json::json!({
                            "index": i,
                            "kind": layer_kind_name(l),
                        })
                    })
                    .collect();
                Ok(serde_json::json!({"layers": summaries}))
            }
            "camera.frame" => {
                let p: CameraFrameParams = parse_params(&req.params)?;
                let cmd = CameraCommand::Frame(p.set);
                self.apply_view_command(ViewCommand::Camera(cmd.clone()));
                let (vw, vh) = self.last_viewport.unwrap_or((800.0, 600.0));
                self.enact_camera(&cmd, vw, vh);
                Ok(serde_json::json!({"ok": true}))
            }
            "camera.home" => {
                let cmd = CameraCommand::Home;
                self.apply_view_command(ViewCommand::Camera(cmd.clone()));
                let (vw, vh) = self.last_viewport.unwrap_or((800.0, 600.0));
                self.enact_camera(&cmd, vw, vh);
                Ok(serde_json::json!({"ok": true}))
            }
            "query.focus" => {
                Ok(serde_json::json!({"symbol": self.focus.current.qualified_path}))
            }
            "metric.import" => {
                let p: MetricImportParams = parse_params(&req.params)?;
                self.apply_and_respond(ViewCommand::ImportMetric {
                    name: p.name,
                    metric: p.metric,
                })
            }
            "query.metrics" => {
                let p: QueryMetricsParams = parse_params(&req.params)?;
                let id = outrider_view::symbol_id::parse_wire(&p.symbol).map_err(|e| {
                    RpcError {
                        code: -32602,
                        message: format!("invalid symbol: {e}"),
                        data: None,
                    }
                })?;
                let node = find_node(&self.tree.root, &id).ok_or_else(|| RpcError {
                    code: -32602,
                    message: format!("symbol not found: {}", p.symbol),
                    data: None,
                })?;
                let readouts: Vec<serde_json::Value> = self
                    .metrics
                    .readouts(node)
                    .into_iter()
                    .map(|r| {
                        serde_json::json!({
                            "metric": r.metric,
                            "raw": r.raw,
                            "unit": r.unit,
                            "percentile": r.percentile,
                            "basis": r.basis,
                            "text": r.text(),
                        })
                    })
                    .collect();
                Ok(serde_json::json!({"readouts": readouts}))
            }
            "camera.follow" => {
                let p: CameraFollowParams = parse_params(&req.params)?;
                let mode = match p.mode.as_str() {
                    "focus" => outrider_view::spec::FollowMode::Focus,
                    "none" => outrider_view::spec::FollowMode::None,
                    other => {
                        return Err(RpcError {
                            code: -32602,
                            message: format!("unknown follow mode: {other}"),
                            data: None,
                        })
                    }
                };
                self.apply_and_respond(ViewCommand::Camera(CameraCommand::Follow(mode)))
            }
            "tour.add" => {
                let p: TourAddParams = parse_params(&req.params)?;
                self.apply_and_respond(ViewCommand::Tour(TourCommand::Add(p.step)))
            }
            "tour.setSteps" => {
                let p: TourSetStepsParams = parse_params(&req.params)?;
                self.apply_and_respond(ViewCommand::Tour(TourCommand::SetSteps(p.steps)))
            }
            "tour.goto" => {
                let p: TourGotoParams = parse_params(&req.params)?;
                self.apply_and_respond(ViewCommand::Tour(TourCommand::Goto(p.index)))
            }
            "tour.stop" => {
                self.apply_and_respond(ViewCommand::Tour(TourCommand::Stop))
            }
            "tour.play" => {
                self.apply_and_respond(ViewCommand::Tour(TourCommand::Play))
            }
            "tour.next" => {
                self.apply_and_respond(ViewCommand::Tour(TourCommand::Next))
            }
            "tour.prev" => {
                self.apply_and_respond(ViewCommand::Tour(TourCommand::Prev))
            }
            "tour.loadHistory" => {
                let steps = self.nav_history.to_steps();
                self.apply_and_respond(ViewCommand::Tour(TourCommand::SetSteps(steps)))
            }
            "query.camera" => {
                let c = self.camera.unwrap_or(crate::camera::Camera {
                    center_x: 0.0,
                    center_y: 0.0,
                    zoom: 1.0,
                });
                Ok(serde_json::json!({
                    "centerX": c.center_x,
                    "centerY": c.center_y,
                    "zoom": c.zoom,
                    "follow": format!("{:?}", self.view_spec.camera.follow).to_lowercase(),
                    "step": self.view_spec.camera.step,
                    "stepsCount": self.view_spec.camera.steps.len(),
                }))
            }
            _ => Err(RpcError {
                code: -32601,
                message: "method not found".into(),
                data: None,
            }),
        }
    }

    fn apply_and_respond(
        &mut self,
        cmd: ViewCommand,
    ) -> Result<serde_json::Value, RpcError> {
        let applied = self.apply_view_command(cmd);
        if applied.violations.iter().any(|v| v.hard) {
            Err(RpcError {
                code: -32001,
                message: format!("{} violation(s)", applied.violations.len()),
                data: Some(
                    serde_json::to_value(&applied.violations).unwrap_or_default(),
                ),
            })
        } else {
            Ok(serde_json::json!({"ok": true}))
        }
    }
}

// ── Param structs ──

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ViewApplyParams {
    spec: outrider_view::ViewSpec,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ViewPatchParams {
    patch: outrider_view::command::ViewPatch,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ViewClearParams {
    scope: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetDefineParams {
    name: String,
    expr: outrider_view::SetExpr,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayerPushParams {
    layer: outrider_view::LayerSpec,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayerRemoveParams {
    index: usize,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CameraFrameParams {
    set: outrider_view::SetRef,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MetricImportParams {
    name: String,
    metric: outrider_view::ImportedMetric,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryMetricsParams {
    symbol: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CameraFollowParams {
    mode: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TourAddParams {
    step: outrider_view::spec::Step,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TourSetStepsParams {
    steps: Vec<outrider_view::spec::Step>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TourGotoParams {
    index: usize,
}

fn parse_params<T: serde::de::DeserializeOwned>(
    params: &serde_json::Value,
) -> Result<T, RpcError> {
    serde_json::from_value(params.clone()).map_err(|e| RpcError {
        code: -32602,
        message: format!("invalid params: {e}"),
        data: None,
    })
}

fn find_node<'a>(root: &'a SymbolNode, id: &SymbolId) -> Option<&'a SymbolNode> {
    if root.id == *id {
        return Some(root);
    }
    for child in &root.children {
        if let Some(n) = find_node(child, id) {
            return Some(n);
        }
    }
    None
}

fn layer_kind_name(layer: &outrider_view::LayerSpec) -> &'static str {
    match layer {
        outrider_view::LayerSpec::Fill(_) => "fill",
        outrider_view::LayerSpec::Mask(_) => "mask",
        outrider_view::LayerSpec::Edges(_) => "edges",
        outrider_view::LayerSpec::Marks(_) => "marks",
        outrider_view::LayerSpec::Notes(_) => "notes",
        outrider_view::LayerSpec::Panel(_) => "panel",
    }
}
