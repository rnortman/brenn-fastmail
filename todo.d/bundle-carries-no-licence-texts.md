The bundle a deployment installs holds components and specifications and no
licence text: not this project's `LICENSE` and `NOTICE`, and none for the
crates its components link — `serde`, `serde_json` and the rest of brenn's
guest crates, and from this repository's own hub `roxmltree` and `memchr`.
Brenn's bundle rule stages `components/`, `surface/` and `modules/` and
nothing else, so carrying a licence file takes a change to that rule in brenn,
or a decision to ship the texts beside the bundle. Done = a deployer who
installs the bundle has the licence texts of everything in it, or a recorded
decision that the source repository's files suffice. The
`TODO(bundle-carries-no-licence-texts)` comment sits at the bundle target in
`BUILD.bazel`.
