# Makefile Parsing and Syntax Highlighting Design

## Goal

Add structural parsing and syntax highlighting for `Makefile`, `makefile`,
`GNUmakefile`, and `*.mk` files. Make targets become navigable items while every
byte of the source remains represented, including content outside targets.

## Language Recognition

Introduce a shared, path-aware `SourceLanguage` classifier in `outrider-index`.
It recognizes existing languages by extension and Make from the three
conventional extensionless filenames plus the `.mk` extension. Structural
indexing and buffer highlighting both consume this classifier so their language
support cannot drift apart.

Filename matching is exact for `Makefile`, `makefile`, and `GNUmakefile`.
Extension matching follows the repository's existing extension conventions.

## Tree-sitter Integration

Use the community Tree-sitter Make grammar for parsing. Keep the highlight query
in Outrider so capture names and palette mappings remain under project control.
The query covers comments, targets, variables, functions, strings, automatic
variables, directives, operators, and recipe text where the grammar exposes
stable nodes.

Grammar or query errors degrade to plain source presentation. They must not
prevent the file from being indexed or displayed.

## Structural Extraction

The file node remains the parent. Its children are ordered, non-overlapping
source regions of two kinds:

- `target`: an explicit, pattern, static-pattern, or multi-target rule. Its
  range includes the rule header and recipe. A multi-target rule remains one
  item labeled with the target list as written, avoiding overlapping ranges.
- `section`: a contiguous range not covered by a target. Conservative labels
  include `Variables`, `Includes`, `Conditionals`, `Definitions`, `Preamble`,
  and the neutral fallback `Section`.

Leading comments attach to a target only when the parse tree provides a reliable
association. Otherwise they remain in the neighboring section. Blank lines are
assigned deterministically to adjacent regions.

After target extraction, a coverage pass fills every gap from byte zero through
end-of-file with section items. Children therefore cover the complete file
without overlaps or missing bytes. Malformed syntax remains inside a section; a
complete parse failure yields one full-file section.

Nested conditionals and directives stay within their containing target or
section. The first version does not create a deep Make-specific hierarchy for
individual assignments, directives, or recipe lines.

## Data Flow

1. The scanner retains the file using its real relative path.
2. The shared classifier maps the path to `SourceLanguage::Make`.
3. The indexer parses the bytes and builds ordered target and section items.
4. The renderer materializes the same path, selects the Make grammar and query,
   and produces per-line highlight spans.
5. Existing layout and rendering consume the resulting ordinary symbol tree and
   `FileBuffer`; they require no Make-specific branches.

## Error Handling

- Unsupported or malformed constructs are represented as source sections.
- Individual unnamed rules use a stable textual fallback label.
- Invalid UTF-8 continues to follow the repository's existing file-loading
  behavior.
- Highlight-query compilation errors return the existing buffer error and allow
  the UI's current detail fallback.

## Testing

Tests are added before implementation and cover:

- Recognition of all four filename forms and rejection of lookalikes.
- Target extraction for explicit, pattern, multi-target, and recipe-bearing
  rules.
- Preservation and labeling of assignments, includes, comments, conditionals,
  and preamble content outside targets.
- A coverage invariant: sorted child ranges start at zero, do not overlap or
  gap, and end at the source length.
- Malformed input falling back to retained sections.
- Highlight captures for representative Make syntax.
- End-to-end indexing and buffer materialization for extensionless and `.mk`
  files.

The focused crate tests and workspace-level checks appropriate to the changed
code provide the completion gate.

## Non-goals

- Evaluating Make variables or includes.
- Expanding generated target names.
- Parsing shell recipe bodies with a second grammar.
- Call-graph extraction from recipes.
- Creating child nodes for every assignment, directive, or recipe command.
