# cdknuke

- Removes cdk.out directories from AWS CDK projects throughout a repository. Uses the same intelligent
  directory scanning as nodenuke. Supports `--no-root` flag to start from current directory instead of
  git root, and `--hidden` flag to include hidden directories in the search.
- To install: `cargo install --git https://github.com/timmattison/tools cdknuke`
