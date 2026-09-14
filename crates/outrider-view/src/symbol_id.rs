//! Wire-format serialization for `SymbolId`.

use outrider_index::{SymbolId, SymbolKind};

/// Convert a `SymbolId` to its wire string: `"<kind>:<qualified_path>[#<ordinal>]"`.
pub fn to_wire(id: &SymbolId) -> String {
    let kind_str = match &id.kind {
        SymbolKind::Folder => "folder",
        SymbolKind::File => "file",
        SymbolKind::Chunk => "chunk",
        SymbolKind::Item { label } => label.as_str(),
    };
    if id.ordinal == 0 {
        format!("{kind_str}:{}", id.qualified_path)
    } else {
        format!("{kind_str}:{}#{}", id.qualified_path, id.ordinal)
    }
}

/// Parse a wire string back to `SymbolId`.
/// Accepts `"kind:path"`, `"kind:path#ordinal"`, or bare `"path"` (kind defaults to File).
pub fn parse_wire(s: &str) -> Result<SymbolId, String> {
    // Try kind:path[#ord] format
    if let Some((kind_str, rest)) = s.split_once(':') {
        let (path, ordinal) = parse_ordinal(rest);
        let kind = parse_kind(kind_str)?;
        Ok(SymbolId {
            kind,
            qualified_path: path.to_string(),
            ordinal,
        })
    } else {
        // Bare path — default to File
        let (path, ordinal) = parse_ordinal(s);
        Ok(SymbolId {
            kind: SymbolKind::File,
            qualified_path: path.to_string(),
            ordinal,
        })
    }
}

fn parse_ordinal(s: &str) -> (&str, u16) {
    if let Some((path, ord_str)) = s.rsplit_once('#') {
        if let Ok(ord) = ord_str.parse::<u16>() {
            return (path, ord);
        }
    }
    (s, 0)
}

fn parse_kind(s: &str) -> Result<SymbolKind, String> {
    match s {
        "folder" => Ok(SymbolKind::Folder),
        "file" => Ok(SymbolKind::File),
        "chunk" => Ok(SymbolKind::Chunk),
        other => Ok(SymbolKind::Item {
            label: other.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_file_id() {
        let id = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/a.rs".to_string(),
            ordinal: 0,
        };
        let wire = to_wire(&id);
        assert_eq!(wire, "file:src/a.rs");
        assert_eq!(parse_wire(&wire).unwrap(), id);
    }

    #[test]
    fn round_trips_item_id_with_ordinal() {
        let id = SymbolId {
            kind: SymbolKind::Item {
                label: "fn".to_string(),
            },
            qualified_path: "src/a.rs::foo".to_string(),
            ordinal: 2,
        };
        let wire = to_wire(&id);
        assert_eq!(wire, "fn:src/a.rs::foo#2");
        assert_eq!(parse_wire(&wire).unwrap(), id);
    }

    #[test]
    fn bare_path_defaults_to_file() {
        let id = parse_wire("src/a.rs").unwrap();
        assert_eq!(id.kind, SymbolKind::File);
        assert_eq!(id.qualified_path, "src/a.rs");
        assert_eq!(id.ordinal, 0);
    }
}
