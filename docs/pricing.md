# Pricing data

The dated API-equivalent price table lives in [`crates/semon-sessions/src/pricing.json`](../crates/semon-sessions/src/pricing.json). `pricing.rs` parses it once at runtime; the Rust code keeps the existing four-field compatibility output while using the full rates for cost calculation.

The weekly pricing workflow reads the official Anthropic and OpenAI pricing pages. It opens or updates one review PR when a price or model changes. If a page cannot be parsed, it leaves the table alone and opens or updates a `pricing` issue with the error. Saved trimmed HTML fixtures exercise the parsers offline; the generator refreshes them together with the table when prices or models change.

The two OpenAI compatibility rows that are not on the official page are marked `sync: false` and keep their legacy display rates; their cost-calculation rates stay unset until the page verifies them.
