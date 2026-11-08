# shellcheck shell=sh
# Run a command with every GIT_ variable removed from its environment.
#
# Git exports GIT_DIR, GIT_INDEX_FILE and more into each hook, and every
# program that the hook starts inherits them. The rule is the whole GIT_
# prefix. A list of names goes stale the day git adds a variable.
#
# The function uses a subshell. The git commands of the hook itself keep the
# index that git is about to commit, because the unset happens only inside it.
#
# Husky starts each hook with `sh -e`, so this function must work in plain
# POSIX sh. Do not use `compgen`. It is a bash builtin. Under a shell that
# lacks it, a loop over `$(compgen -e)` iterates over nothing and removes
# nothing.
#
# A line inside a multi-line value can look like a name. That is harmless,
# because the only effect is an `unset` of a GIT_ name.
#
# Usage: without_git_environment cargo test --workspace
without_git_environment() {
    (
        for name in $(env | sed -n 's/^\(GIT_[A-Za-z0-9_]*\)=.*/\1/p'); do
            unset "$name"
        done
        exec "$@"
    )
}
