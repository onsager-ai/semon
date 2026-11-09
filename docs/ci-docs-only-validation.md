# Docs-only CI validation

This temporary, unmerged change validates the docs-only path introduced by PR #589.
The pull request targets `ci/docs-only`, so its complete diff contains only this
Markdown file. Rust and Viewer aggregate checks must succeed while the expensive
suite jobs skip. The validation pull request will be closed after its CI evidence
is recorded; this file is not intended to land on main.
