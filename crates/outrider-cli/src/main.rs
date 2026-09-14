mod cli;
mod compile;
mod exit;
mod instance;
mod rpc;

use clap::Parser;
use serde_json::json;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let cli = match cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(exit::BAD_ARGS);
        }
    };

    match run(cli) {
        Ok(()) => ExitCode::from(exit::OK),
        Err(e) => e,
    }
}

fn run(cli: cli::Cli) -> Result<(), ExitCode> {
    let root = instance::find_project_root(cli.project.as_deref()).map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(exit::NO_INSTANCE)
    })?;

    let inst = instance::load_instance(&root).map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(exit::NO_INSTANCE)
    })?;

    let timeout = Duration::from_millis(cli.timeout);
    let mut client = rpc::Client::connect(inst.port, &inst.token, timeout).map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(exit::NO_INSTANCE)
    })?;

    let is_json = cli.json || std::env::var("OUTRIDER_JSON").is_ok();

    match cli.command {
        cli::Command::Set { name, args } => {
            let expr = compile::set_expr(&args).map_err(|e| {
                eprintln!("{e}");
                ExitCode::from(exit::BAD_ARGS)
            })?;
            let result = call_or_fail(
                &mut client,
                "set.define",
                json!({"name": name, "expr": expr}),
                is_json,
            )?;
            if !is_json {
                println!("ok · set \"{name}\"");
            } else {
                println!("{}", serde_json::to_string_pretty(&result).unwrap());
            }
        }
        cli::Command::Mask {
            dim_except,
            dim,
            strength,
        } => {
            let layer = compile::mask_layer(&dim_except, &dim, strength).map_err(|e| {
                eprintln!("{e}");
                ExitCode::from(exit::BAD_ARGS)
            })?;
            let result = call_or_fail(
                &mut client,
                "layer.push",
                json!({"layer": layer}),
                is_json,
            )?;
            if !is_json {
                println!("ok · mask");
            } else {
                println!("{}", serde_json::to_string_pretty(&result).unwrap());
            }
        }
        cli::Command::Frame { set } => {
            let result = call_or_fail(
                &mut client,
                "camera.frame",
                json!({"set": set}),
                is_json,
            )?;
            if !is_json {
                println!("ok · frame \"{set}\"");
            } else {
                println!("{}", serde_json::to_string_pretty(&result).unwrap());
            }
        }
        cli::Command::Home => {
            let result = call_or_fail(&mut client, "camera.home", json!({}), is_json)?;
            if !is_json {
                println!("ok");
            } else {
                println!("{}", serde_json::to_string_pretty(&result).unwrap());
            }
        }
        cli::Command::View { action } => match action {
            cli::ViewAction::Apply { file } => {
                let content = if file == "-" {
                    use std::io::Read;
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s).map_err(|e| {
                        eprintln!("cannot read stdin: {e}");
                        ExitCode::from(exit::BAD_ARGS)
                    })?;
                    s
                } else {
                    std::fs::read_to_string(&file).map_err(|e| {
                        eprintln!("cannot read {file}: {e}");
                        ExitCode::from(exit::BAD_ARGS)
                    })?
                };
                let spec: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
                    eprintln!("invalid JSON: {e}");
                    ExitCode::from(exit::BAD_ARGS)
                })?;
                let result = call_or_fail(
                    &mut client,
                    "view.apply",
                    json!({"spec": spec}),
                    is_json,
                )?;
                if !is_json {
                    println!("ok · view applied");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::ViewAction::Get { o } => {
                let result = call_or_fail(&mut client, "view.get", json!({}), is_json)?;
                let spec = result.get("spec").unwrap_or(&result);
                let formatted = serde_json::to_string_pretty(spec).unwrap();
                if let Some(path) = o {
                    std::fs::write(&path, &formatted).map_err(|e| {
                        eprintln!("cannot write {path}: {e}");
                        ExitCode::from(exit::RPC_ERROR)
                    })?;
                    if !is_json {
                        println!("wrote {path}");
                    }
                } else {
                    println!("{formatted}");
                }
            }
            cli::ViewAction::Clear { layers: _, sets, all } => {
                let scope = if all {
                    "all"
                } else if sets {
                    "sets"
                } else {
                    "layers"
                };
                let result = call_or_fail(
                    &mut client,
                    "view.clear",
                    json!({"scope": scope}),
                    is_json,
                )?;
                if !is_json {
                    println!("ok · cleared {scope}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
        },
        cli::Command::Layer { action } => match action {
            cli::LayerAction::List => {
                let result = call_or_fail(&mut client, "layer.list", json!({}), is_json)?;
                if is_json {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                } else if let Some(layers) = result.get("layers").and_then(|l| l.as_array()) {
                    for l in layers {
                        let idx = l.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                        let kind = l.get("kind").and_then(|k| k.as_str()).unwrap_or("?");
                        let summary = l.get("summary").and_then(|s| s.as_str()).unwrap_or("");
                        println!("[{idx}] {kind}: {summary}");
                    }
                }
            }
            cli::LayerAction::Pop => {
                let result = call_or_fail(&mut client, "layer.pop", json!({}), is_json)?;
                if !is_json {
                    println!("ok · layer popped");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::LayerAction::Rm { index } => {
                let result = call_or_fail(
                    &mut client,
                    "layer.remove",
                    json!({"index": index}),
                    is_json,
                )?;
                if !is_json {
                    println!("ok · layer {index} removed");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
        },
        cli::Command::Note { symbol, text } => {
            let layer = compile::note_layer(&symbol, &text).map_err(|e| {
                eprintln!("{e}");
                ExitCode::from(exit::BAD_ARGS)
            })?;
            let result = call_or_fail(
                &mut client,
                "layer.push",
                json!({"layer": layer}),
                is_json,
            )?;
            if !is_json {
                println!("ok · note on {symbol}");
            } else {
                println!("{}", serde_json::to_string_pretty(&result).unwrap());
            }
        }
        cli::Command::Panel {
            set,
            columns,
            sort,
            dock,
            title,
            limit,
            id,
        } => {
            let layer = compile::panel_layer(&set, &columns, &sort, &dock, &title, limit, &id)
                .map_err(|e| {
                    eprintln!("{e}");
                    ExitCode::from(exit::BAD_ARGS)
                })?;
            let result = call_or_fail(
                &mut client,
                "layer.push",
                json!({"layer": layer}),
                is_json,
            )?;
            if !is_json {
                println!("ok · panel for \"{set}\"");
            } else {
                println!("{}", serde_json::to_string_pretty(&result).unwrap());
            }
        }
        cli::Command::Fill {
            metric,
            channel,
            scale,
            ramp,
            domain,
        } => {
            let layer = compile::fill_layer(&metric, &channel, &scale, &ramp, &domain)
                .map_err(|e| {
                    eprintln!("{e}");
                    ExitCode::from(exit::BAD_ARGS)
                })?;
            let result = call_or_fail(
                &mut client,
                "layer.push",
                json!({"layer": layer}),
                is_json,
            )?;
            if !is_json {
                println!("ok · fill {metric}");
            } else {
                println!("{}", serde_json::to_string_pretty(&result).unwrap());
            }
        }
        cli::Command::Metric { action } => match action {
            cli::MetricAction::Import { name, file } => {
                let content = if file == "-" {
                    use std::io::Read;
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s).map_err(|e| {
                        eprintln!("cannot read stdin: {e}");
                        ExitCode::from(exit::BAD_ARGS)
                    })?;
                    s
                } else {
                    std::fs::read_to_string(&file).map_err(|e| {
                        eprintln!("cannot read {file}: {e}");
                        ExitCode::from(exit::BAD_ARGS)
                    })?
                };
                let metric: serde_json::Value =
                    serde_json::from_str(&content).map_err(|e| {
                        eprintln!("invalid JSON: {e}");
                        ExitCode::from(exit::BAD_ARGS)
                    })?;
                let result = call_or_fail(
                    &mut client,
                    "metric.import",
                    json!({"name": name, "metric": metric}),
                    is_json,
                )?;
                if !is_json {
                    let resolved = result
                        .get("resolved")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let unresolved = result
                        .get("unresolved")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    println!("ok · imported \"{name}\" ({resolved} resolved, {unresolved} unresolved)");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
        },
        cli::Command::Query { action } => match action {
            cli::QueryAction::Symbols { query, kind, limit } => {
                let mut params = json!({"query": query});
                if let Some(k) = kind {
                    params["kind"] = json!(k);
                }
                if let Some(l) = limit {
                    params["limit"] = json!(l);
                }
                let result = call_or_fail(&mut client, "query.symbols", params, is_json)?;
                if is_json {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                } else if let Some(syms) = result.get("symbols").and_then(|r| r.as_array()) {
                    for s in syms {
                        let id = s.get("id").and_then(|t| t.as_str()).unwrap_or("?");
                        let sig = s
                            .get("signature")
                            .and_then(|t| t.as_str())
                            .map(|t| format!("   {t}"))
                            .unwrap_or_default();
                        println!("{id}{sig}");
                    }
                    let total = result.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
                    if total as usize > syms.len() {
                        println!("({} of {} shown; use --limit)", syms.len(), total);
                    }
                    if syms.is_empty() {
                        println!("(no symbols match)");
                    }
                }
            }
            cli::QueryAction::Metrics { symbol } => {
                let result = call_or_fail(
                    &mut client,
                    "query.metrics",
                    json!({"symbol": symbol}),
                    is_json,
                )?;
                if is_json {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                } else if let Some(readouts) = result.get("readouts").and_then(|r| r.as_array()) {
                    for r in readouts {
                        let text = r.get("text").and_then(|t| t.as_str()).unwrap_or("?");
                        println!("{text}");
                    }
                    if readouts.is_empty() {
                        println!("(no metrics for this symbol)");
                    }
                }
            }
        },
        cli::Command::Tour { action } => match action {
            cli::TourAction::Add { step } => {
                let step_val: serde_json::Value =
                    serde_json::from_str(&step).map_err(|e| {
                        eprintln!("invalid step JSON: {e}");
                        ExitCode::from(exit::BAD_ARGS)
                    })?;
                let result = call_or_fail(
                    &mut client,
                    "tour.add",
                    json!({"step": step_val}),
                    is_json,
                )?;
                if !is_json {
                    println!("ok · step added");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::TourAction::Play => {
                let result = call_or_fail(&mut client, "tour.play", json!({}), is_json)?;
                if !is_json {
                    println!("ok · tour playing");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::TourAction::Next => {
                let result = call_or_fail(&mut client, "tour.next", json!({}), is_json)?;
                if !is_json {
                    println!("ok · next step");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::TourAction::Prev => {
                let result = call_or_fail(&mut client, "tour.prev", json!({}), is_json)?;
                if !is_json {
                    println!("ok · previous step");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::TourAction::Goto { index } => {
                let result = call_or_fail(
                    &mut client,
                    "tour.goto",
                    json!({"index": index}),
                    is_json,
                )?;
                if !is_json {
                    println!("ok · goto step {index}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::TourAction::Stop => {
                let result = call_or_fail(&mut client, "tour.stop", json!({}), is_json)?;
                if !is_json {
                    println!("ok · tour stopped");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::TourAction::SetSteps { file } => {
                let content = if file == "-" {
                    use std::io::Read;
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s).map_err(|e| {
                        eprintln!("cannot read stdin: {e}");
                        ExitCode::from(exit::BAD_ARGS)
                    })?;
                    s
                } else {
                    std::fs::read_to_string(&file).map_err(|e| {
                        eprintln!("cannot read {file}: {e}");
                        ExitCode::from(exit::BAD_ARGS)
                    })?
                };
                let steps: serde_json::Value =
                    serde_json::from_str(&content).map_err(|e| {
                        eprintln!("invalid JSON: {e}");
                        ExitCode::from(exit::BAD_ARGS)
                    })?;
                let result = call_or_fail(
                    &mut client,
                    "tour.setSteps",
                    json!({"steps": steps}),
                    is_json,
                )?;
                if !is_json {
                    println!("ok · steps loaded");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::TourAction::LoadHistory => {
                let result = call_or_fail(
                    &mut client,
                    "tour.loadHistory",
                    json!({}),
                    is_json,
                )?;
                if !is_json {
                    println!("ok · loaded navigation history as tour");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
        },
        cli::Command::Camera { action } => match action {
            cli::CameraAction::Follow { mode } => {
                let result = call_or_fail(
                    &mut client,
                    "camera.follow",
                    json!({"mode": mode}),
                    is_json,
                )?;
                if !is_json {
                    println!("ok · follow mode: {mode}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
        },
        cli::Command::Comments { action } => match action {
            cli::CommentsAction::List => {
                let result = call_or_fail(&mut client, "comments.list", json!({}), is_json)?;
                if is_json {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                } else {
                    let comments = result
                        .get("comments")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    if comments.is_empty() {
                        println!("no comments");
                    } else {
                        for c in &comments {
                            let id = c.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                            let label = c
                                .get("label")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let target = c
                                .get("target")
                                .and_then(|v| v.as_str())
                                .unwrap_or("(general)");
                            let text = c.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            println!("[{id}] {label} \u{2014} {target}");
                            for line in text.lines() {
                                println!("    {line}");
                            }
                        }
                        println!(
                            "\nuse `comments prompt` for the ready-to-paste agent prompt"
                        );
                    }
                }
            }
            cli::CommentsAction::Prompt => {
                let result = call_or_fail(&mut client, "comments.list", json!({}), is_json)?;
                match result.get("prompt").and_then(|v| v.as_str()) {
                    Some(p) => println!("{p}"),
                    None => println!("no comments"),
                }
            }
            cli::CommentsAction::Remove { id } => {
                let result =
                    call_or_fail(&mut client, "comments.remove", json!({"id": id}), is_json)?;
                if !is_json {
                    let removed = result
                        .get("removed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    println!(
                        "{}",
                        if removed {
                            format!("ok \u{00B7} removed comment {id}")
                        } else {
                            format!("no comment with id {id}")
                        }
                    );
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
            cli::CommentsAction::Clear => {
                let result = call_or_fail(&mut client, "comments.clear", json!({}), is_json)?;
                if !is_json {
                    let n = result.get("cleared").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("ok \u{00B7} cleared {n} comment(s)");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
            }
        },
        cli::Command::Status => {
            if !is_json {
                println!("connected to outrider on port {}", inst.port);
                println!("project: {}", inst.project_root);
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "port": inst.port,
                        "project_root": inst.project_root,
                        "status": "connected"
                    }))
                    .unwrap()
                );
            }
        }
    }

    Ok(())
}

fn call_or_fail(
    client: &mut rpc::Client,
    method: &str,
    params: serde_json::Value,
    is_json: bool,
) -> Result<serde_json::Value, ExitCode> {
    match client.call(method, params) {
        Ok(result) => Ok(result),
        Err(rpc::RpcFailure::Violation { violations }) => {
            if is_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"error": "violation", "violations": violations})
                    )
                    .unwrap()
                );
            } else {
                for v in &violations {
                    let path = v.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                    let msg = v.get("message").and_then(|v| v.as_str()).unwrap_or("?");
                    let rule = v.get("rule").and_then(|v| v.as_str()).unwrap_or("?");
                    eprintln!("error: {path} -- {msg} ({rule})");
                }
            }
            Err(ExitCode::from(exit::VIOLATION))
        }
        Err(rpc::RpcFailure::Rpc {
            code, message, ..
        }) => {
            if is_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"error": "rpc", "code": code, "message": message})
                    )
                    .unwrap()
                );
            } else {
                eprintln!("error: {message} (code {code})");
            }
            Err(ExitCode::from(exit::RPC_ERROR))
        }
        Err(rpc::RpcFailure::Io(e)) => {
            eprintln!("connection error: {e}");
            Err(ExitCode::from(exit::RPC_ERROR))
        }
    }
}
