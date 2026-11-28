# aa

- AWS Account - quickly get AWS account information without a pager. Runs the equivalent of
  `aws sts get-caller-identity` but as a simple Rust binary that outputs JSON directly to stdout.
  Perfect for when you need to check which AWS account you're using frequently and don't want to
  type the full AWS CLI command or deal with pager output.
- To install: `cargo install --git https://github.com/timmattison/tools aa`
