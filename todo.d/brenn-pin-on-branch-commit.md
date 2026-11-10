`MODULE.bazel` pins a brenn commit that brenn's public remote serves from a
branch, not from `main`. If that branch is deleted or rewritten before `main`
carries the commit, the anonymous fetch may fail and every `bazel` invocation
with it. Done = the pin names a commit on brenn's `main`. The comment sits at
the pin in `MODULE.bazel`.
