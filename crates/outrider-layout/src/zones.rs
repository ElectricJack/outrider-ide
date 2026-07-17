use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolKind, SymbolNode};

use crate::progressive::PackCancelled;

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SemanticRole {
    Source,
    Test,
    Example,
    ShaderAsset,
    Docs,
    Generated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RoleProfile {
    pub role: SemanticRole,
    pub strong: bool,
}

pub(crate) type RoleProfiles = BTreeMap<SymbolId, RoleProfile>;

const ROLES_BY_PRECEDENCE: [SemanticRole; 6] = [
    SemanticRole::Generated,
    SemanticRole::Test,
    SemanticRole::Example,
    SemanticRole::ShaderAsset,
    SemanticRole::Docs,
    SemanticRole::Source,
];

fn normalized(name: &str) -> (Vec<String>, String) {
    let lower = name.to_ascii_lowercase();
    let tokens = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect();
    let collapsed = lower.chars().filter(char::is_ascii_alphanumeric).collect();
    (tokens, collapsed)
}

fn has_signal(tokens: &[String], collapsed: &str, signals: &[&str]) -> bool {
    signals
        .iter()
        .any(|signal| tokens.iter().any(|token| token == *signal) || collapsed == *signal)
}

fn explicit_folder_role(name: &str) -> Option<SemanticRole> {
    let (tokens, collapsed) = normalized(name);
    ROLES_BY_PRECEDENCE.into_iter().find(|role| match role {
        SemanticRole::Generated => has_signal(
            &tokens,
            &collapsed,
            &[
                "generated",
                "vendor",
                "vendors",
                "thirdparty",
                "external",
                "extern",
                "deps",
                "dependencies",
            ],
        ),
        SemanticRole::Test => has_signal(
            &tokens,
            &collapsed,
            &["test", "tests", "testing", "spec", "specs"],
        ),
        SemanticRole::Example => has_signal(
            &tokens,
            &collapsed,
            &["example", "examples", "demo", "demos", "sample", "samples"],
        ),
        SemanticRole::ShaderAsset => has_signal(
            &tokens,
            &collapsed,
            &[
                "shader",
                "shaders",
                "asset",
                "assets",
                "resource",
                "resources",
                "texture",
                "textures",
                "model",
                "models",
                "media",
            ],
        ),
        SemanticRole::Docs => has_signal(&tokens, &collapsed, &["doc", "docs", "documentation"]),
        SemanticRole::Source => has_signal(
            &tokens,
            &collapsed,
            &[
                "src", "source", "sources", "include", "includes", "lib", "libs", "core",
            ],
        ),
    })
}

fn classify_file(name: &str) -> RoleProfile {
    let (stem, extension) = name
        .rsplit_once('.')
        .map(|(stem, ext)| (stem, Some(ext.to_ascii_lowercase())))
        .unwrap_or((name, None));
    let (tokens, collapsed) = normalized(stem);
    let role = ROLES_BY_PRECEDENCE
        .into_iter()
        .find(|role| file_matches(*role, &tokens, &collapsed, extension.as_deref()))
        .unwrap_or(SemanticRole::Source);
    RoleProfile {
        role,
        strong: role != SemanticRole::Source,
    }
}

fn file_matches(
    role: SemanticRole,
    tokens: &[String],
    collapsed: &str,
    extension: Option<&str>,
) -> bool {
    match role {
        SemanticRole::Generated => {
            has_signal(tokens, collapsed, &["generated", "vendor", "thirdparty"])
        }
        SemanticRole::Test => has_signal(tokens, collapsed, &["test", "tests", "spec", "specs"]),
        SemanticRole::Example => has_signal(tokens, collapsed, &["example", "demo", "sample"]),
        SemanticRole::ShaderAsset => extension.is_some_and(|extension| {
            matches!(
                extension,
                "glsl"
                    | "vert"
                    | "frag"
                    | "geom"
                    | "comp"
                    | "tesc"
                    | "tese"
                    | "rgen"
                    | "rchit"
                    | "rmiss"
                    | "rahit"
                    | "rint"
                    | "rcall"
                    | "mesh"
                    | "task"
                    | "hlsl"
                    | "fx"
                    | "fxh"
                    | "metal"
                    | "wgsl"
                    | "spv"
                    | "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "bmp"
                    | "tga"
                    | "hdr"
                    | "exr"
                    | "dds"
                    | "ktx"
                    | "ktx2"
                    | "svg"
                    | "ico"
                    | "obj"
                    | "fbx"
                    | "gltf"
                    | "glb"
                    | "dae"
                    | "ply"
                    | "stl"
                    | "wav"
                    | "mp3"
                    | "ogg"
                    | "flac"
                    | "mp4"
                    | "mov"
                    | "webm"
            )
        }),
        SemanticRole::Docs => extension
            .is_some_and(|extension| matches!(extension, "md" | "markdown" | "txt" | "rst")),
        SemanticRole::Source => false,
    }
}

#[derive(Default)]
struct RoleCounts {
    files: [u64; 6],
}

impl RoleCounts {
    fn add(&mut self, other: &Self) {
        for (left, right) in self.files.iter_mut().zip(other.files) {
            *left += right;
        }
    }

    fn total(&self) -> u64 {
        self.files.iter().sum()
    }

    fn dominant_non_source(&self) -> Option<SemanticRole> {
        let total = self.total();
        ROLES_BY_PRECEDENCE
            .into_iter()
            .filter(|role| *role != SemanticRole::Source)
            .find(|role| self.files[*role as usize] * 10 >= total * 7 && total > 0)
    }
}

pub(crate) fn build_profiles(root: &SymbolNode) -> RoleProfiles {
    build_profiles_cancellable(root, &|| false).expect("never-cancel profile build")
}

pub(crate) fn build_profiles_cancellable<C>(
    root: &SymbolNode,
    is_cancelled: &C,
) -> Result<RoleProfiles, PackCancelled>
where
    C: Fn() -> bool,
{
    let mut profiles = BTreeMap::new();
    collect_profiles(root, &mut profiles, is_cancelled)?;
    Ok(profiles)
}

fn collect_profiles<C>(
    node: &SymbolNode,
    profiles: &mut RoleProfiles,
    is_cancelled: &C,
) -> Result<RoleCounts, PackCancelled>
where
    C: Fn() -> bool,
{
    if is_cancelled() {
        return Err(PackCancelled);
    }
    match &node.id.kind {
        SymbolKind::File => {
            let profile = classify_file(&node.name);
            profiles.insert(node.id.clone(), profile);
            let mut counts = RoleCounts::default();
            counts.files[profile.role as usize] = 1;
            Ok(counts)
        }
        SymbolKind::Folder => {
            let mut counts = RoleCounts::default();
            for child in &node.children {
                counts.add(&collect_profiles(child, profiles, is_cancelled)?);
            }
            let profile = explicit_folder_role(&node.name)
                .map(|role| RoleProfile { role, strong: true })
                .or_else(|| {
                    counts
                        .dominant_non_source()
                        .map(|role| RoleProfile { role, strong: true })
                })
                .unwrap_or(RoleProfile {
                    role: SemanticRole::Source,
                    strong: false,
                });
            profiles.insert(node.id.clone(), profile);
            Ok(counts)
        }
        _ => {
            profiles.insert(
                node.id.clone(),
                RoleProfile {
                    role: SemanticRole::Source,
                    strong: false,
                },
            );
            Ok(RoleCounts::default())
        }
    }
}

pub(crate) fn effective_role(
    id: &SymbolId,
    inherited: SemanticRole,
    profiles: &RoleProfiles,
) -> SemanticRole {
    let profile = profiles.get(id).copied().unwrap_or(RoleProfile {
        role: SemanticRole::Source,
        strong: false,
    });
    if profile.strong {
        profile.role
    } else {
        inherited
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    fn node(kind: SymbolKind, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind,
                qualified_path: name.into(),
                ordinal: 0,
            },
            name: name.into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    #[test]
    fn explicit_folder_roles_cover_all_zones() {
        let cases = [
            ("src", SemanticRole::Source),
            ("unit_tests", SemanticRole::Test),
            ("Demo-Samples", SemanticRole::Example),
            ("ray_shaders", SemanticRole::ShaderAsset),
            ("Documentation", SemanticRole::Docs),
            ("third_party", SemanticRole::Generated),
        ];
        for (name, expected) in cases {
            assert_eq!(explicit_folder_role(name), Some(expected), "{name}");
        }
    }

    #[test]
    fn generated_precedence_wins_ambiguous_names() {
        assert_eq!(
            explicit_folder_role("generated_test_assets"),
            Some(SemanticRole::Generated)
        );
    }

    #[test]
    fn file_roles_cover_suffixes_and_extensions() {
        let cases = [
            ("mesh_test.cpp", SemanticRole::Test),
            ("lighting.demo.rs", SemanticRole::Example),
            ("closest_hit.rchit", SemanticRole::ShaderAsset),
            ("README.md", SemanticRole::Docs),
            ("ordinary.cpp", SemanticRole::Source),
        ];
        for (name, expected) in cases {
            assert_eq!(classify_file(name).role, expected, "{name}");
        }
    }

    #[test]
    fn generated_file_stem_punctuation_variants_share_role() {
        for name in ["third_party.rs", "third-party.rs", "thirdparty.rs"] {
            assert_eq!(
                classify_file(name),
                RoleProfile {
                    role: SemanticRole::Generated,
                    strong: true,
                },
                "{name}"
            );
        }
    }

    #[test]
    fn explicitly_named_folder_is_strong_regardless_of_descendants() {
        let ordinary = node(SymbolKind::File, "ordinary.cpp", vec![]);
        let folder = node(SymbolKind::Folder, "tests", vec![ordinary]);
        let profiles = build_profiles(&folder);

        assert_eq!(
            profiles[&folder.id],
            RoleProfile {
                role: SemanticRole::Test,
                strong: true,
            }
        );
    }

    #[test]
    fn seventy_percent_non_source_descendants_make_a_strong_folder() {
        let files = (0..7)
            .map(|i| node(SymbolKind::File, &format!("case_{i}_test.cpp"), vec![]))
            .chain((0..3).map(|i| node(SymbolKind::File, &format!("impl_{i}.cpp"), vec![])))
            .collect();
        let folder = node(SymbolKind::Folder, "unit", files);
        let profiles = build_profiles(&folder);
        assert_eq!(
            profiles[&folder.id],
            RoleProfile {
                role: SemanticRole::Test,
                strong: true,
            }
        );
    }

    #[test]
    fn ordinary_source_folder_is_weak_and_inherits_test_context() {
        let file = node(SymbolKind::File, "ordinary.cpp", vec![]);
        let folder = node(SymbolKind::Folder, "unit", vec![file]);
        let profiles = build_profiles(&folder);
        assert!(!profiles[&folder.id].strong);
        assert_eq!(
            effective_role(&folder.id, SemanticRole::Test, &profiles),
            SemanticRole::Test
        );
    }

    #[test]
    fn mixed_folder_falls_back_to_weak_source() {
        let files = (0..6)
            .map(|i| node(SymbolKind::File, &format!("case_{i}_test.cpp"), vec![]))
            .chain((0..4).map(|i| node(SymbolKind::File, &format!("guide_{i}.md"), vec![])))
            .collect();
        let folder = node(SymbolKind::Folder, "unit", files);
        let profiles = build_profiles(&folder);

        assert_eq!(
            profiles[&folder.id],
            RoleProfile {
                role: SemanticRole::Source,
                strong: false,
            }
        );
    }

    #[test]
    fn strong_file_role_overrides_inherited_context() {
        let file = node(SymbolKind::File, "guide.md", vec![]);
        let profiles = build_profiles(&file);

        assert_eq!(
            effective_role(&file.id, SemanticRole::Test, &profiles),
            SemanticRole::Docs
        );
    }
}
