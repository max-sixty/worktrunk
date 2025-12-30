# Twitter Thread: Worktrunk Launch

<!--
STRUCTURE:
1. Intro - hook with announcement (link-free for max reach)
2. Context - Claude Code instances, why isolation matters
3. Git worktree UX problem
4. What Worktrunk is + GitHub link
5. Core command - wt switch
6. Other core commands - list, remove
7-12. Features - hooks, list + CI status, select, LLM commits, wt merge, status line
13. Omnibus demo - full workflow with parallel agents in Zellij
14. CTA - install, docs, feedback, star
15. Thanks to Claude Code team + RT request

DESIGN DECISIONS:
- Lead with "models improved → running more agents → worktrees → UX terrible → built this"
- 🧵 signals it's a thread
- Tweet 1 is link-free (hook needs max reach)
- GitHub link in tweet 4 (what Worktrunk is); GitHub star in tweet 14 (CTA)
- Tweet 2 explains isolation need; tweet 3 shows UX pain; tweet 4 introduces Worktrunk; tweet 5 shows wt switch
- Tweet 15 thanks team + RT request
- Core commands split across two tweets for focused demos
- Features are snappy, one per tweet, with media where applicable
- Monospace: Twitter doesn't support it; use screenshots or plain text (GIF shows commands)
- Social proof (PRQL, xarray, insta) cut for now to keep focus on the tool
-->

---

<!-- ============ PHASE 1: INTRO ============ -->
<!-- Goal: Hook, announce, explain what it is, set up the thread -->

<!-- TODO: Wordsmith tweets 1-2. Current version is functional but doesn't sing.
     Attempted combining them but lost clarity. Key requirements:
     - Tweet 1 must say what Worktrunk is (git worktree manager)
     - Need to convey the AI agent use case without being abstract
     - Lead with concrete pain, not marketing speak
     - Avoid slop: "fills the gap", "painful UX", "actually usable" -->

**1/** (190 chars)
Announcing Worktrunk! A git worktree manager, designed for running AI agents in parallel.

A few points on why I'm so excited about the project, and why I hope it becomes broadly adopted 🧵

[wt-core.gif](../../../assets/wt-core.gif)

<!-- NOTE: Considered Zellij demo here but it's too complex for tweet 1's hook role.
     Placed omnibus demo at tweet 13 instead (before CTA). -->

<!-- ============ PHASE 2: CONTEXT ============ -->
<!-- Goal: Why isolation matters, then prove the UX problem -->

**2/** (202 chars)
As models have improved this year, I've been running more & more Claude Code instances in parallel, often 5-10.

Each needs its own isolated working directory, otherwise they get confused by each other's changes.

**3/** (222 chars)
Git worktrees solve this, but the UX is terrible!

To create & navigate to a new worktree in git:

𝚐𝚒𝚝 𝚠𝚘𝚛𝚔𝚝𝚛𝚎𝚎 𝚊𝚍𝚍 -𝚋 𝚏𝚎𝚊𝚝 ../𝚛𝚎𝚙𝚘.𝚏𝚎𝚊𝚝 && 𝚌𝚍 ../𝚛𝚎𝚙𝚘.𝚏𝚎𝚊𝚝

...even for a simple command, we need to type the name three times.

<!-- ============ PHASE 3: CORE COMMANDS ============ -->
<!-- Goal: Contrast with solution, then introduce core commands -->

**4/** (167 chars)
Worktrunk is a CLI, written in Rust, which makes git worktrees as easy as branches.

https://github.com/max-sixty/worktrunk

**5/** (99 chars)
In contrast to the git command, the Worktrunk command to create a new worktree is short (& aliasable):

𝚠𝚝 𝚜𝚠𝚒𝚝𝚌𝚑 --𝚌𝚛𝚎𝚊𝚝𝚎 𝚊𝚙𝚒

[wt-switch.gif](../../../assets/wt-switch.gif)

**6/** (105 chars)
Worktrunk's other core commands:

𝚠𝚝 𝚕𝚒𝚜𝚝: see all worktrees with status
𝚠𝚝 𝚛𝚎𝚖𝚘𝚟𝚎: delete a worktree

[wt-list-remove.gif](../../../assets/wt-list-remove.gif)

<!-- ============ PHASE 4: FEATURES ============ -->
<!-- Goal: List additional capabilities, one per tweet, snappy -->

**7/** (275 chars)
Beyond core commands, Worktrunk has quality-of-life features to simplify working with many parallel changes:

Hooks: Post-start hooks run after creating a worktree: install deps, copy caches, start dev servers, etc. And there's a hook for every stage of a worktree lifecycle.

[wt-hooks.gif](../../../assets/wt-hooks.gif)

<!-- TODO: Consider cutting or merging tweets 8-9. Reviewers noted:
     - "50ms" is too technical / doesn't connect to AI workflows
     - Fuzzy picker isn't differentiated (every CLI has one)
     - Thread may be too long; these are weak candidates for cutting -->

**8/** (235 chars)
𝚠𝚝 𝚕𝚒𝚜𝚝 renders in ~50ms, then fills in details (CI status, diff stats) as they become available. Can also list branches with 𝚠𝚝 𝚕𝚒𝚜𝚝 --𝚋𝚛𝚊𝚗𝚌𝚑𝚎𝚜.

𝚠𝚝 𝚕𝚒𝚜𝚝 --𝚏𝚞𝚕𝚕: CI status as clickable dots. Green/blue/red. Clicking opens the PR.

[wt-list.gif](../../../assets/wt-list.gif)

**9/** (45 chars)
𝚠𝚝 𝚜𝚎𝚕𝚎𝚌𝚝: fuzzy picker across all branches.

[wt-select-short.gif](../../../assets/wt-select-short.gif)

**10/** (99 chars)
LLM Commits: When running 𝚠𝚝 𝚜𝚝𝚎𝚙 𝚌𝚘𝚖𝚖𝚒𝚝 or 𝚠𝚝 𝚖𝚎𝚛𝚐𝚎, worktrunk can have an LLM write the commit message, with a customizable template.

[wt-commit.gif](../../../assets/wt-commit.gif)

**11/** (78 chars)
𝚠𝚝 𝚖𝚎𝚛𝚐𝚎: squash, rebase, merge, remove worktree, delete branch, in one command.

[wt-merge.gif](../../../assets/wt-merge.gif)

**12/** (83 chars)
@claudeai status line integration. See branch, diff stats, CI status at a glance.

[wt-statusline.gif](../../../assets/wt-statusline.gif)

**13/** (168 chars)
Putting it all together: parallel Claude Code agents in Zellij tabs, each in its own worktree. The full lifecycle: 𝚠𝚝 𝚜𝚠𝚒𝚝𝚌𝚑, 𝚠𝚝 𝚕𝚒𝚜𝚝, 𝚠𝚝 𝚜𝚎𝚕𝚎𝚌𝚝, 𝚠𝚝 𝚖𝚎𝚛𝚐𝚎.

[wt-zellij-omnibus.gif](../../../assets/wt-zellij-omnibus.gif)

<!-- ============ PHASE 5: CTA ============ -->
<!-- Goal: Install instructions, docs, invite feedback, star -->

**14/** (167 chars)
To install:

𝚋𝚛𝚎𝚠 𝚒𝚗𝚜𝚝𝚊𝚕𝚕 𝚖𝚊𝚡-𝚜𝚒𝚡𝚝𝚢/𝚠𝚘𝚛𝚔𝚝𝚛𝚞𝚗𝚔/𝚠𝚝
𝚠𝚝 𝚌𝚘𝚗𝚏𝚒𝚐 𝚜𝚑𝚎𝚕𝚕 𝚒𝚗𝚜𝚝𝚊𝚕𝚕

Feedback welcome. Open an issue or reply here.

⭐ https://github.com/max-sixty/worktrunk

**15/** (230 chars)
Big thanks to @AnthropicAI and the @claudeai team, including @bcherny @\_catwu @alexalbert\_\_, for building Claude Code. Worktrunk wouldn't exist without it 🙏

If this was useful, liking & RT-ing the first tweet helps spread the word.

[TODO: paste link to tweet 1]

---

## Notes

- **Monospace in tweets**: Twitter doesn't support code formatting. Options:
  - Unicode monospace via [YayText](https://yaytext.com/monospace/): 𝚠𝚝 𝚜𝚠𝚒𝚝𝚌𝚑 -𝚌 𝚏𝚎𝚊𝚝
  - Screenshots
  - Plain text (the GIF shows commands anyway)
- **Social proof**: Cut for now, could add back in a later tweet
