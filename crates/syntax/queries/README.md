# Highlight queries

The grammars' own queries come with their crates; these are the ones a
crate doesn't export:

- `julia.scm`, `dockerfile.scm`, `proto.scm`: copied from the
  `tree-sitter-julia` 0.23.1, `tree-sitter-dockerfile-updated` 0.2.0 and
  `tree-sitter-proto` 0.6.0 crates' `queries/highlights.scm` (MIT, as
  their crates are), unchanged.
- `kotlin.scm`: Olo's own (`tree-sitter-kotlin-ng` ships none).
