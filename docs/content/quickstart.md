+++
title = "Quick Start"
weight = 2
+++

## Install

**Homebrew (macOS):**

```bash
$ brew install max-sixty/worktrunk/wt
$ wt config shell install  # allows commands to change directories
```

**Cargo:**

```bash
$ cargo install worktrunk
$ wt config shell install
```

## Create a worktree

<!-- ⚠️ AUTO-GENERATED-HTML from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_simple_switch.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> wt switch --create fix-auth
✅ <span style='color:var(--green,#0a0)'>Created new worktree for <b>fix-auth</b> from <b>main</b> at <b>../repo.fix-auth</b></span>
{% end %}

<!-- END AUTO-GENERATED -->

This creates `../repo.fix-auth` on branch `fix-auth`.

## Switch between worktrees

<!-- ⚠️ AUTO-GENERATED-HTML from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_switch_back.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> wt switch feature-api
✅ <span style='color:var(--green,#0a0)'>Switched to worktree for <b>feature-api</b> at <b>../repo.feature-api</b></span>
{% end %}

<!-- END AUTO-GENERATED -->

## List worktrees

<!-- ⚠️ AUTO-GENERATED-HTML from tests/snapshots/integration__integration_tests__list__readme_example_simple_list.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> wt list
  <b>Branch</b>       <b>Status</b>         <b>HEAD±</b>    <b>main↕</b>  <b>Path</b>                <b>Remote⇅</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ <b>feature-api</b>  <span style='color:var(--cyan,#0aa)'>+</span>   <span style='opacity:0.67'>↑</span><span style='opacity:0.67'>⇡</span>      <span style='color:var(--green,#0a0)'>+36</span>  <span style='color:var(--red,#a00)'>-11</span>   <span style='color:var(--green,#0a0)'>↑4</span>      <b>./repo.feature-api</b>   <span style='color:var(--green,#0a0)'>⇡3</span>      <span style='opacity:0.67'>b1554967</span>  <span style='opacity:0.67'>30m</span>   <span style='opacity:0.67'>Add API tests</span>
^ main             <span style='opacity:0.67'>^</span><span style='opacity:0.67'>⇣</span>                         ./repo                   <span style='opacity:0.67'><span style='color:var(--red,#a00)'>⇣1</span></span>  <span style='opacity:0.67'>b834638e</span>  <span style='opacity:0.67'>1d</span>    <span style='opacity:0.67'>Initial commit</span>
+ <span style='opacity:0.67'>fix-auth</span>        <span style='opacity:0.67'>_</span>                           <span style='opacity:0.67'>./repo.fix-auth</span>              <span style='opacity:0.67'>b834638e</span>  <span style='opacity:0.67'>1d</span>    <span style='opacity:0.67'>Initial commit</span>

⚪ <span style='opacity:0.67'>Showing 3 worktrees, 1 with changes, 1 ahead</span>
{% end %}

<!-- END AUTO-GENERATED -->

Add `--full` for CI status and conflicts. Add `--branches` to include all branches.

## Clean up

When you're done with a worktree (e.g., after merging via CI):

<!-- ⚠️ AUTO-GENERATED-HTML from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_remove.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> wt remove
🔄 <span style='color:var(--cyan,#0aa)'>Removing <b>feature-api</b> worktree &amp; branch in background (already in main)</span>
{% end %}

<!-- END AUTO-GENERATED -->

Worktrunk checks if your changes are already on main before deleting the branch.

<!-- TODO: Add shortcuts (@, -, ^) somewhere more prominent in the docs -
     too early for quickstart but should be discoverable -->

## Next steps

- Understand [why worktrees matter](/concepts/) and how Worktrunk improves on plain git
- Set up [project hooks](/configuration/) for automated setup
- Use [LLM commit messages](/configuration/#llm-commit-messages) for auto-generated commits
