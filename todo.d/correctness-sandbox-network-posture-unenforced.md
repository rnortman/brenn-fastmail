Make the gate enforce the sandboxed, network-less build posture `.bazelrc`
asks for. Today nothing checks it, and three ordinary paths break it while
`make check` stays green. First, a `requires-network` tag or execution
requirement on any action overrides `--sandbox_default_allow_network=false`.
Second, a test tagged `no-sandbox` or `local` falls back silently to the
`local` strategy under `--strategy=TestRunner=sandboxed,local`, with host
network. A brenn macro instantiated in `BUILD.bazel` could start carrying
such a tag at a pin bump. Third, on a host without Bazel's Linux sandbox
(no unprivileged user namespaces), `sandboxed` resolves to the
processwrapper sandbox, which isolates neither the network nor the source
tree. Candidate mechanisms: a `bazel query` step in `check` that requires
the set of targets tagged `no-sandbox`, `local` or `requires-network` to be
exactly `//:buildifier_test`; a canary `sh_test` that passes only if a TCP
connect through bash's `/dev/tcp` fails, which covers the third path on
whatever host runs the gate, CI included. Other repositories built on the
same scaffold share the posture and the gap, so the choice is best made once
for all of them. Done means: `make check` fails when an action other than
`//:buildifier_test` runs unsandboxed or with network. The
`TODO(correctness-sandbox-network-posture-unenforced)` comment sits above
`check` in the `Makefile`.
