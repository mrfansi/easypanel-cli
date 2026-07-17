**What changed, and why it matters**

<!-- Explain the change and the reason for it, not a diff summary. If it fixes a
bug that could show a wrong number or lose data, say so plainly. -->

**How it was verified**

<!--
Be specific and honest about what was and wasn't checked:
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` all clean
- [ ] Behaviour that only a live API can validate was tested against a real server
      (or: this change needs no live verification, because …)
- [ ] For a TUI change: the binary was run and the screen looked right — tests
      check shapes, not whether a form can be submitted or a column updates

Name anything you could NOT verify. "I didn't test X" is more useful than silence.
-->

**Anything reviewers should know**

<!-- Trade-offs, a corner deliberately cut, a follow-up you're leaving for later. -->
