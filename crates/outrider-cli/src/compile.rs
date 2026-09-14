use serde_json::{json, Value};

/// Compile SetArgs into a SetExpr JSON value.
pub fn set_expr(args: &super::cli::SetArgs) -> Result<Value, String> {
    // --expr is exclusive
    if let Some(raw) = &args.expr {
        if !args.glob.is_empty()
            || !args.kind.is_empty()
            || args.fuzzy.is_some()
            || !args.ids.is_empty()
            || args.from_set.is_some()
        {
            return Err("--expr is mutually exclusive with other set flags".into());
        }
        return serde_json::from_str(raw).map_err(|e| format!("invalid --expr JSON: {e}"));
    }

    let mut primaries: Vec<Value> = Vec::new();

    for g in &args.glob {
        primaries.push(json!({"glob": g}));
    }
    for k in &args.kind {
        primaries.push(json!({"kind": k}));
    }
    if let Some(q) = &args.fuzzy {
        primaries.push(json!({"fuzzy": q}));
    }
    if !args.ids.is_empty() {
        primaries.push(json!({"ids": args.ids}));
    }
    if let Some(name) = &args.from_set {
        primaries.push(json!({"ref": name}));
    }

    if primaries.is_empty() {
        return Err(
            "set needs at least one selector (--glob, --kind, --fuzzy, --ids, --from, or --expr)"
                .into(),
        );
    }

    if primaries.len() == 1 {
        Ok(primaries.into_iter().next().unwrap())
    } else {
        Ok(json!({"intersect": primaries}))
    }
}

/// Compile mask flags into a LayerSpec::Mask JSON value.
pub fn mask_layer(
    dim_except: &Option<String>,
    dim: &Option<String>,
    strength: f32,
) -> Result<Value, String> {
    match (dim_except, dim) {
        (Some(_), Some(_)) => Err("--dim-except and --dim are mutually exclusive".into()),
        (None, None) => Err("mask requires --dim-except or --dim".into()),
        (Some(set), None) => Ok(json!({"mask": {"dimExcept": set, "strength": strength}})),
        (None, Some(set)) => Ok(json!({"mask": {"dim": set, "strength": strength}})),
    }
}

pub fn note_layer(symbol_spec: &str, text: &str) -> Result<Value, String> {
    let (symbol, lines) = if let Some(colon_pos) = symbol_spec.rfind(':') {
        let after = &symbol_spec[colon_pos + 1..];
        if let Some((start, end)) = after.split_once('-') {
            match (start.parse::<usize>(), end.parse::<usize>()) {
                (Ok(s), Ok(e)) => (&symbol_spec[..colon_pos], Some([s, e])),
                _ => (symbol_spec, None),
            }
        } else {
            (symbol_spec, None)
        }
    } else {
        (symbol_spec, None)
    };

    let at = if let Some(lines) = lines {
        json!({"symbol": symbol, "lines": lines})
    } else {
        json!(symbol)
    };

    Ok(json!({
        "notes": [{
            "at": at,
            "source": "agent",
            "text": text,
        }]
    }))
}

/// Compile panel CLI args into a LayerSpec::Panel JSON value.
pub fn panel_layer(
    set: &str,
    columns: &Option<String>,
    sort: &Option<String>,
    dock: &str,
    title: &Option<String>,
    limit: Option<usize>,
    id: &Option<String>,
) -> Result<Value, String> {
    let dock_val = match dock.to_ascii_lowercase().as_str() {
        "left" => "left",
        "right" => "right",
        "bottom" => "bottom",
        "float" => "float",
        other => return Err(format!("unknown dock position: {other} (expected left, right, bottom, or float)")),
    };

    let sort_by: Option<Value> = sort.as_ref().map(|s| match s.as_str() {
        "name" => json!("name"),
        "nameLength" => json!("nameLength"),
        metric => json!(metric),
    });

    let cols: Vec<Value> = columns
        .as_ref()
        .map(|c| c.split(',').map(|s| json!(s.trim())).collect())
        .unwrap_or_default();

    let mut panel = json!({
        "rows": {"set": set},
        "dock": dock_val,
    });

    let obj = panel.as_object_mut().unwrap();
    if !cols.is_empty() {
        obj.insert("columns".into(), json!(cols));
    }
    if let Some(sb) = sort_by {
        obj.insert("sortBy".into(), sb);
    }
    if let Some(t) = title {
        obj.insert("title".into(), json!(t));
    }
    if let Some(l) = limit {
        obj.insert("limit".into(), json!(l));
    }
    if let Some(i) = id {
        obj.insert("id".into(), json!(i));
    }

    Ok(json!({"panel": panel}))
}

pub fn fill_layer(
    metric: &str,
    channel: &str,
    scale: &str,
    ramp: &Option<String>,
    domain: &Option<String>,
) -> Result<Value, String> {
    let channel_val = match channel.to_ascii_lowercase().as_str() {
        "fill" => "fill",
        "stripe" => "stripe",
        "opacity" => "opacity",
        other => return Err(format!("unknown fill channel: {other} (expected fill, stripe, or opacity)")),
    };

    let scale_val = match scale.to_ascii_lowercase().as_str() {
        "linear" => json!("linear"),
        "log" => json!("log"),
        "percentile" => json!("percentile"),
        "categorical" => json!("categorical"),
        other => return Err(format!("unknown scale: {other} (expected linear, log, percentile, or categorical)")),
    };

    let mut fill = json!({
        "metric": metric,
        "channel": channel_val,
        "scale": scale_val,
    });

    let obj = fill.as_object_mut().unwrap();
    if let Some(r) = ramp {
        obj.insert("ramp".into(), json!(r));
    }
    if let Some(d) = domain {
        obj.insert("domain".into(), json!(d));
    }

    Ok(json!({"fill": fill}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::SetArgs;

    fn make_set_args(
        glob: Vec<&str>,
        kind: Vec<&str>,
        fuzzy: Option<&str>,
        ids: Vec<&str>,
        from_set: Option<&str>,
        expr: Option<&str>,
    ) -> SetArgs {
        SetArgs {
            glob: glob.into_iter().map(String::from).collect(),
            kind: kind.into_iter().map(String::from).collect(),
            fuzzy: fuzzy.map(String::from),
            ids: ids.into_iter().map(String::from).collect(),
            from_set: from_set.map(String::from),
            expr: expr.map(String::from),
        }
    }

    #[test]
    fn single_glob() {
        let args = make_set_args(vec!["src/**"], vec![], None, vec![], None, None);
        let result = set_expr(&args).unwrap();
        assert_eq!(result, json!({"glob": "src/**"}));
    }

    #[test]
    fn two_globs_intersect() {
        let args = make_set_args(vec!["a", "b"], vec![], None, vec![], None, None);
        let result = set_expr(&args).unwrap();
        assert_eq!(result, json!({"intersect": [{"glob": "a"}, {"glob": "b"}]}));
    }

    #[test]
    fn glob_and_kind_intersect() {
        let args = make_set_args(vec!["src/**"], vec!["fn"], None, vec![], None, None);
        let result = set_expr(&args).unwrap();
        assert_eq!(
            result,
            json!({"intersect": [{"glob": "src/**"}, {"kind": "fn"}]})
        );
    }

    #[test]
    fn no_primaries_is_error() {
        let args = make_set_args(vec![], vec![], None, vec![], None, None);
        assert!(set_expr(&args).is_err());
    }

    #[test]
    fn expr_exclusive_with_glob() {
        let args = make_set_args(vec!["x"], vec![], None, vec![], None, Some(r#"{"kind":"fn"}"#));
        assert!(set_expr(&args).is_err());
    }

    #[test]
    fn mask_both_dim_and_dim_except_error() {
        let result = mask_layer(
            &Some("a".into()),
            &Some("b".into()),
            0.8,
        );
        assert!(result.is_err());
    }

    #[test]
    fn mask_neither_dim_nor_dim_except_error() {
        let result = mask_layer(&None, &None, 0.8);
        assert!(result.is_err());
    }

    #[test]
    fn mask_dim_except() {
        let result = mask_layer(&Some("s".into()), &None, 0.8).unwrap();
        let mask = result.get("mask").expect("missing mask key");
        assert_eq!(mask.get("dimExcept").unwrap(), "s");
        let strength = mask.get("strength").unwrap().as_f64().unwrap();
        assert!((strength - 0.8).abs() < 1e-6, "strength {strength} not close to 0.8");
    }

    #[test]
    fn note_symbol_only() {
        let result = note_layer("fn:src/a.rs::foo", "hello").unwrap();
        let notes = result.get("notes").unwrap().as_array().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0]["at"], json!("fn:src/a.rs::foo"));
        assert_eq!(notes[0]["source"], "agent");
        assert_eq!(notes[0]["text"], "hello");
    }

    #[test]
    fn note_with_line_range() {
        let result = note_layer("fn:src/a.rs::foo:40-52", "new call").unwrap();
        let notes = result.get("notes").unwrap().as_array().unwrap();
        assert_eq!(notes[0]["at"]["symbol"], "fn:src/a.rs::foo");
        assert_eq!(notes[0]["at"]["lines"], json!([40, 52]));
    }

    #[test]
    fn fill_basic() {
        let result = fill_layer("churn", "fill", "percentile", &None, &None).unwrap();
        let fill = result.get("fill").expect("missing fill key");
        assert_eq!(fill.get("metric").unwrap(), "churn");
        assert_eq!(fill.get("channel").unwrap(), "fill");
        assert_eq!(fill.get("scale").unwrap(), "percentile");
        assert!(fill.get("ramp").is_none());
        assert!(fill.get("domain").is_none());
    }

    #[test]
    fn fill_with_ramp_and_domain() {
        let result = fill_layer(
            "complexity",
            "stripe",
            "log",
            &Some("viridis".into()),
            &Some("hotFns".into()),
        )
        .unwrap();
        let fill = result.get("fill").unwrap();
        assert_eq!(fill.get("metric").unwrap(), "complexity");
        assert_eq!(fill.get("channel").unwrap(), "stripe");
        assert_eq!(fill.get("scale").unwrap(), "log");
        assert_eq!(fill.get("ramp").unwrap(), "viridis");
        assert_eq!(fill.get("domain").unwrap(), "hotFns");
    }

    #[test]
    fn fill_bad_channel() {
        assert!(fill_layer("churn", "bogus", "linear", &None, &None).is_err());
    }

    #[test]
    fn fill_bad_scale() {
        assert!(fill_layer("churn", "fill", "bogus", &None, &None).is_err());
    }
}
