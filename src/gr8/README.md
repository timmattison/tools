# gr8

- Displays GitHub API rate limit information in a user-friendly format. Fetches rate limits using the GitHub CLI
  (`gh api rate_limit`), converts epoch timestamps to local time in ISO 8601 format, and color-codes the output
  (green for healthy, yellow for under 20% remaining, red for exceeded). Shows limits for all API resource types
  including core, GraphQL, search, code scanning, and more. Requires GitHub CLI to be installed and authenticated.
- To install: `cargo install --git https://github.com/timmattison/tools gr8`
