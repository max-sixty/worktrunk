<!-- markdownlint-disable MD033 -->

[English](README.md) | **Português (Brasil)** | [Español](README.es.md)

<h1><img src="docs/public/logo.png" alt="Logo do Worktrunk" width="50" align="absmiddle">&nbsp;&nbsp;Worktrunk</h1>

[![Docs](https://img.shields.io/badge/docs-worktrunk.dev-blue?style=for-the-badge&logo=gitbook)](https://worktrunk.dev)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=for-the-badge)](https://opensource.org/licenses/MIT)
[![CI](https://img.shields.io/github/actions/workflow/status/max-sixty/worktrunk/ci.yaml?event=push&branch=main&style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/actions?query=branch%3Amain+workflow%3Aci)
[![Codecov](https://img.shields.io/codecov/c/github/max-sixty/worktrunk?style=for-the-badge&logo=codecov)](https://codecov.io/gh/max-sixty/worktrunk)
[![Stars](https://img.shields.io/github/stars/max-sixty/worktrunk?style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/stargazers)
[![maintained with tend](https://img.shields.io/badge/maintained_with-tend-bba580?style=for-the-badge&logo=data:image/svg%2bxml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxNiAxNiI+PGcgdHJhbnNmb3JtPSJ0cmFuc2xhdGUoMCwxNikgc2NhbGUoMC4wMTI1LC0wLjAxMjUpIiBmaWxsPSIjZmZmIiBzdHJva2U9Im5vbmUiPjxwYXRoIGQ9Ik02ODAgMTEyOCBjNjIgLTk2IDY5IC0xNzggMjAgLTI0MSAtMTcgLTIyIC0yMCAtNDAgLTIwIC0xMzQgbDEgLTEwOCAyMSAyOCBjMTEgMTYgMzAgNDcgNDIgNzAgMTIgMjIgMzIgNDkgNDYgNTkgMzcgMjcgMTE0IDM4IDE4NCAyNyA5MyAtMTUgOTQgLTE4IDQ0IC03OSAtNzIgLTg4IC0xMDkgLTExMyAtMTc2IC0xMTcgLTMxIC0yIC02NCAxIC03MiA2IC0yMyAxNSAyMSA1NiAxMDcgOTggNDAgMjAgNzEgMzggNjkgNDAgLTYgNyAtODggLTE3IC0xMjYgLTM3IC00OSAtMjUgLTEwMCAtNzggLTEyMSAtMTI1IC0xNSAtMzMgLTE5IC02NiAtMTkgLTE4OCAwIC0xNTcgOCAtMTk1IDUwIC0yMzIgMTcgLTE2IDM2IC0yMCA4NSAtMTkgNjIgMSA2MyAxIDczIC0zMiA5IC0zMiA5IC0zMyAtMjIgLTQwIC01MCAtMTIgLTEzMiAtNyAtMTY0IDEwIC00MCAyMSAtNzkgNjkgLTkyIDExNCAtNSAyMCAtMTAgMTAyIC0xMCAxODIgMCA4MCAtNSAxNjIgLTExIDE4NCAtMjIgNzkgLTEzNSAxNjYgLTIzNCAxODEgLTM3IDYgLTM1IDMgMzAgLTI4IDc4IC0zOSAxNDQgLTkxIDEzMiAtMTA0IC01IC00IC0zNyAtOCAtNzEgLTggLTc3IDAgLTExNyAyNCAtMTgyIDEwOSAtNTIgNjggLTUxIDcwIDQyIDg1IDcxIDExIDE0MyAwIDE4MyAtMjkgMTYgLTExIDQwIC00MyA1NCAtNzMgMTMgLTI5IDMyIC01OSA0MSAtNjYgMTQgLTEyIDE2IC03IDE2IDU4IDAgNTkgNCA3NyAyMyAxMDIgMTkgMjYgMjMgNDYgMjUgMTMwIDMgNjcgMCA5OSAtNyA5OSAtNyAwIC0xMSAtMjMgLTEyIC01NyAwIC0zMiAtNiAtNzYgLTEyIC05NyBsLTEyIC00MCAtMjcgMzIgYy0zNCA0MSAtNDMgOTYgLTI0IDE1MSAxNCA0MSA3NSAxNDEgODYgMTQxIDMgMCAyMSAtMjQgNDAgLTUyeiIvPjwvZz48L3N2Zz4K)](https://github.com/max-sixty/tend)

> **Novembro de 2026**: o Worktrunk foi [lançado](https://x.com/max_sixty/status/2006077845391724739?s=20) no início do ano e rapidamente se tornou o gerenciador de worktrees do git mais popular. Ele é feito com carinho (nada de slop!). Por favor, me conte qualquer atrito, por menor que seja; estou intensamente focado em continuar deixando o Worktrunk excelente, e a maior ajuda é a galera relatando os problemas que percebe.

O Worktrunk é uma CLI para gerenciar worktrees do git, pensada para rodar agentes de IA em paralelo.

Os três comandos principais do Worktrunk deixam as worktrees tão simples quanto branches. Além disso, o Worktrunk traz uma porção de recursos de conveniência para facilitar o trabalho com muitas mudanças em paralelo, incluindo hooks para automatizar fluxos locais e caches de build com copy-on-write.

Uma demonstração rápida:

![Demonstração do Worktrunk](https://cdn.jsdelivr.net/gh/max-sixty/worktrunk-assets@main/assets/docs/light/wt-core.gif)

> ### 📚 Documentação completa em [worktrunk.dev](https://worktrunk.dev) 📚

## Contexto: worktrees do git

Agentes de IA como o Claude Code e o Codex conseguem dar conta de tarefas mais
longas sem supervisão, a ponto de ser possível tocar de 5 a 10 ou mais em
paralelo. O recurso nativo de worktree do git dá a cada agente seu próprio
diretório de trabalho, para que eles não atrapalhem as mudanças uns dos outros.

Mas a experiência de uso das worktrees do git é desajeitada. Até uma tarefa tão
pequena quanto iniciar uma nova worktree exige digitar o nome da branch três
vezes: `git worktree add -b feat ../repo.feat` e depois `cd ../repo.feat`.

## O Worktrunk deixa as worktrees do git tão simples quanto branches

As worktrees são endereçadas pelo nome da branch; os caminhos são calculados a partir de um template configurável. Comandos que recebem uma branch também aceitam o caminho da worktree em que ela está checada.

<p class="workflow-stage">Comece pelos comandos principais</p>

**Comandos principais:**

<table class="cmd-compare">
  <thead>
    <tr>
      <th>Tarefa</th>
      <th>Worktrunk</th>
      <th>Git puro</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td>Trocar de worktree</td>
      <td data-label="Worktrunk"><code>wt switch feat</code></td>
      <td data-label="Git puro"><code>cd ../repo.feat</code></td>
    </tr>
    <tr>
      <td>Criar + iniciar o Claude</td>
      <td data-label="Worktrunk"><code>wt switch -c -x claude feat</code></td>
      <td data-label="Git puro"><pre><code>git worktree add -b feat ../repo.feat && \
cd ../repo.feat && \
claude</code></pre></td>
    </tr>
    <tr>
      <td>Limpar</td>
      <td data-label="Worktrunk"><code>wt remove</code></td>
      <td data-label="Git puro"><pre><code>cd ../repo && \
git worktree remove ../repo.feat && \
git branch -d feat</code></pre></td>
    </tr>
    <tr>
      <td>Listar com status</td>
      <td data-label="Worktrunk"><code>wt list</code></td>
      <td data-label="Git puro"><span class="cmd-compare-value"><code>git worktree list</code> (só os caminhos)</span></td>
    </tr>
  </tbody>
</table>

<p class="workflow-stage">Avance para os comandos mais avançados conforme a necessidade</p>

<p class="workflow-heading"><strong>Automação de fluxo de trabalho:</strong></p>

- **[Hooks](https://worktrunk.dev/hook/)** — rode comandos na criação, antes do merge, depois do merge etc.
- **[Mensagens de commit por LLM](https://worktrunk.dev/llm-commits/)** — gere mensagens de commit a partir dos diffs
- **[Fluxo de merge](https://worktrunk.dev/merge/)** — squash, rebase, merge e limpeza em um único comando
- **[Seletor interativo](https://worktrunk.dev/switch/#interactive-picker)** — navegue pelas worktrees com status de CI em tempo real e prévias de diff, log, PR e comentários
- **[Caches de build compartilhados](https://worktrunk.dev/step/#wt-step-copy-ignored)** — dez worktrees ganham `target/`, `node_modules/` etc. sem precisar compilar nem copiar (em APFS, btrfs e XFS)
- **[`wt list --full`](https://worktrunk.dev/list/#full-mode)** — [status de CI](https://worktrunk.dev/list/#ci-status) e [resumos gerados por IA](https://worktrunk.dev/list/#llm-summaries) por branch
- **[Checkout de PR](https://worktrunk.dev/switch/#pull-requests-and-merge-requests)** — `wt switch pr:123` para pular direto para a branch de um PR
- **[Servidor de desenvolvimento por worktree](https://worktrunk.dev/tips-patterns/#dev-server-per-worktree)** — o filtro de template `hash_port` dá a cada worktree uma porta única
- **[Aliases](https://worktrunk.dev/extending/#aliases) e [variáveis por branch](https://worktrunk.dev/config/#wt-config-state-vars)** — comandos `wt <nome>` personalizados e estado com escopo de branch para templates de hook
- ...e **[muito mais](#próximos-passos)**

Vários agentes em paralelo, os mesmos comandos simples:

![Demonstração completa do Worktrunk: vários agentes Claude em abas do Zellij com hooks, commits por LLM e fluxo de merge](https://raw.githubusercontent.com/max-sixty/worktrunk-assets/main/assets/docs/light/wt-zellij-omnibus.gif)

## Instalação

**Homebrew (macOS e Linux):**

```bash
brew install worktrunk && wt config shell install
```

A integração com o shell permite que os comandos troquem de diretório.

**Cargo:**

```bash
cargo install worktrunk && wt config shell install
```

<details>
<summary><strong>Windows e outros</strong></summary>

**Windows.** O `wt` é, por padrão, o comando do Windows Terminal, então o Winget instala adicionalmente o Worktrunk como `git-wt` para evitar o conflito:

```bash
winget install max-sixty.worktrunk
git-wt config shell install
```

Como alternativa, desative o alias do Windows Terminal (Configurações → Aplicativos → Configurações avançadas de aplicativos → Aliases de execução de aplicativo → "Terminal"/"Terminal Preview") para usar o `wt` diretamente.

> Assinatura de código gratuita fornecida pela [SignPath.io](https://signpath.io/), certificado da [SignPath Foundation](https://signpath.org/) — [política](https://worktrunk.dev/code-signing/).

**Arch Linux:**

```bash
sudo pacman -S worktrunk && wt config shell install
```

**Conda / Pixi** ([feedstock](https://github.com/conda-forge/worktrunk-feedstock) mantido pela comunidade):

```bash
conda install -c conda-forge worktrunk && wt config shell install
```

Ou com o [Pixi](https://pixi.sh): `pixi global install worktrunk && wt config shell install`.

</details>

## Início rápido

Crie uma worktree para uma nova funcionalidade:

```console
$ wt switch --create feature-auth
✓ Created branch feature-auth from main and worktree @ ~/repo.feature-auth
```

Isso cria uma nova branch e worktree e troca para ela. Faça o seu trabalho e então confira todas as worktrees com o [`wt list`](https://worktrunk.dev/list/):

```console
$ wt list
  Branch        Status      HEAD±     main↕    main…±    Remote⇅  Commit    Age  Message
@ feature-auth  +   ↑      +27   -8   ↑1       +31                4bc72dc    2h  Add authenticatio…
^ main              ^⇡                                    ⇡1      0e631ad    1d  Initial commit

○ Showing 2 worktrees, 1 with changes, 1 ahead, hidden: Path
```

O `@` marca a worktree atual. `+` indica mudanças no stage, `↑1` indica 1 commit à frente da main e `⇡` indica commits não enviados.

Ao terminar, escolha entre:

**Fluxo com PR** — commitar, dar push, abrir um PR, fazer merge pelo GitHub/GitLab e então limpar:

```bash
wt step commit                    # commita as mudanças no stage
gh pr create                      # ou glab mr create
wt remove                         # depois que o PR for mergeado
```

**Merge local** — squash, rebase sobre a main, merge fast-forward e limpeza:

```console
$ wt merge main
◎ Generating commit message and committing changes... (2 files, +53, no squashing needed)
  Add authentication module
✓ Committed changes @ a1b2c3d
◎ Merging 1 commit to main @ a1b2c3d (no rebase needed)
  * a1b2c3d Add authentication module
   auth.rs | 51 +++++++++++++++++++++++++++++++++++++++++++++++++++
   lib.rs  |  2 ++
   2 files changed, 53 insertions(+)
✓ Merged to main (1 commit, 2 files, +53)
◎ Removing feature-auth worktree & branch in background (same commit as main, _)
○ Switched to worktree for main @ ~/repo
```

Para agentes em paralelo, crie várias worktrees e dispare um agente em cada uma:

```bash
wt switch -x claude -c feature-a -- 'Add user authentication'
wt switch -x claude -c feature-b -- 'Fix the pagination bug'
wt switch -x claude -c feature-c -- 'Write tests for the API'
```

A flag `-x` roda um comando após a troca; os argumentos depois de `--` são repassados a ele. Configure [hooks de pós-início](https://worktrunk.dev/hook/#hook-types) para automatizar a preparação (instalar dependências, subir servidores de desenvolvimento).

## Próximos passos

- Aprenda os comandos principais: [`wt switch`](https://worktrunk.dev/switch/), [`wt list`](https://worktrunk.dev/list/), [`wt merge`](https://worktrunk.dev/merge/), [`wt remove`](https://worktrunk.dev/remove/)
- Configure [hooks](https://worktrunk.dev/hook/) para preparar tudo automaticamente
- Explore [mensagens de commit por LLM](https://worktrunk.dev/llm-commits/), o [seletor
  interativo](https://worktrunk.dev/switch/#interactive-picker), a [integração com o Claude Code](https://worktrunk.dev/claude-code/) e o [status de CI
  e links de PR](https://worktrunk.dev/list/#ci-status)
- Navegue por [dicas e padrões](https://worktrunk.dev/tips-patterns/) para receitas: aliases, servidores de desenvolvimento, bancos de dados, repasse entre agentes e mais
- [Estendendo o Worktrunk](https://worktrunk.dev/extending/) — personalize fluxos com hooks e aliases
- Assista ao [vídeo do @DevOpsToolbox sobre o Worktrunk](https://youtu.be/WBQiqr6LevQ?t=345)
- Rode `wt --help` ou `wt <comando> --help` para uma referência rápida da CLI

## Como contribuir

- ⭐ Dê uma estrela no repositório
- Conte para alguém sobre o Worktrunk
- [Abra uma issue](https://github.com/max-sixty/worktrunk/issues/new?title=&body=%23%23%20Description%0A%0A%3C!--%20Describe%20the%20bug%20or%20feature%20request%20--%3E%0A%0A%23%23%20Context%0A%0A%3C!--%20Any%20relevant%20context%3A%20your%20workflow%2C%20what%20you%20were%20trying%20to%20do%2C%20etc.%20--%3E) — feedback, pedidos de funcionalidade, até um pequeno atrito ou uma mensagem de usuário imperfeita, ou [uma dor com worktrees ainda não resolvida](https://github.com/max-sixty/worktrunk/issues/new?title=Worktree%20friction%3A%20&body=%23%23%20The%20friction%0A%0A%3C!--%20What%20worktree-related%20task%20is%20still%20painful%3F%20--%3E%0A%0A%23%23%20Current%20workaround%0A%0A%3C!--%20How%20do%20you%20handle%20this%20today%3F%20--%3E%0A%0A%23%23%20Ideal%20solution%0A%0A%3C!--%20What%20would%20make%20this%20easier%3F%20--%3E)
- Compartilhe: [X](https://twitter.com/intent/tweet?text=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management&url=https%3A%2F%2Fworktrunk.dev) · [Reddit](https://www.reddit.com/submit?url=https%3A%2F%2Fworktrunk.dev&title=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management) · [LinkedIn](https://www.linkedin.com/sharing/share-offsite/?url=https%3A%2F%2Fworktrunk.dev)

> ### 📚 Documentação completa em [worktrunk.dev](https://worktrunk.dev) 📚

### Histórico de estrelas

<a href="https://star-history.com/#max-sixty/worktrunk&Date">
  <img src="https://api.star-history.com/svg?repos=max-sixty/worktrunk&type=Date&sealed_token=2ySbQiVbkVrGmwgDsJya-xr4ApbVVvR0siYI46d22Xj_1kPCcgA9X0YpUGc3__aMuZ0ZAWzG4NBhJtqepYjlkoYrVwmKbgaPmGpNZTCfSyVp8EDA_IXaOOYW2whsOXDAi6g7HD9ezsnqSl58n7AqW2_4IQ4hY2p3h7tNv_3k4am5ASq1NdpbfrOUazCl" width="500" alt="Star History Chart">
</a>
