<!-- markdownlint-disable MD033 -->

[English](README.md) | [Português (Brasil)](README.pt-BR.md) | **Español**

<h1><img src="docs/public/logo.png" alt="Logotipo de Worktrunk" width="50" align="absmiddle">&nbsp;&nbsp;Worktrunk</h1>

[![Docs](https://img.shields.io/badge/docs-worktrunk.dev-blue?style=for-the-badge&logo=gitbook)](https://worktrunk.dev)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=for-the-badge)](https://opensource.org/licenses/MIT)
[![CI](https://img.shields.io/github/actions/workflow/status/max-sixty/worktrunk/ci.yaml?event=push&branch=main&style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/actions?query=branch%3Amain+workflow%3Aci)
[![Codecov](https://img.shields.io/codecov/c/github/max-sixty/worktrunk?style=for-the-badge&logo=codecov)](https://codecov.io/gh/max-sixty/worktrunk)
[![Stars](https://img.shields.io/github/stars/max-sixty/worktrunk?style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/stargazers)
[![maintained with tend](https://img.shields.io/badge/maintained_with-tend-bba580?style=for-the-badge&logo=data:image/svg%2bxml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxNiAxNiI+PGcgdHJhbnNmb3JtPSJ0cmFuc2xhdGUoMCwxNikgc2NhbGUoMC4wMTI1LC0wLjAxMjUpIiBmaWxsPSIjZmZmIiBzdHJva2U9Im5vbmUiPjxwYXRoIGQ9Ik02ODAgMTEyOCBjNjIgLTk2IDY5IC0xNzggMjAgLTI0MSAtMTcgLTIyIC0yMCAtNDAgLTIwIC0xMzQgbDEgLTEwOCAyMSAyOCBjMTEgMTYgMzAgNDcgNDIgNzAgMTIgMjIgMzIgNDkgNDYgNTkgMzcgMjcgMTE0IDM4IDE4NCAyNyA5MyAtMTUgOTQgLTE4IDQ0IC03OSAtNzIgLTg4IC0xMDkgLTExMyAtMTc2IC0xMTcgLTMxIC0yIC02NCAxIC03MiA2IC0yMyAxNSAyMSA1NiAxMDcgOTggNDAgMjAgNzEgMzggNjkgNDAgLTYgNyAtODggLTE3IC0xMjYgLTM3IC00OSAtMjUgLTEwMCAtNzggLTEyMSAtMTI1IC0xNSAtMzMgLTE5IC02NiAtMTkgLTE4OCAwIC0xNTcgOCAtMTk1IDUwIC0yMzIgMTcgLTE2IDM2IC0yMCA4NSAtMTkgNjIgMSA2MyAxIDczIC0zMiA5IC0zMiA5IC0zMyAtMjIgLTQwIC01MCAtMTIgLTEzMiAtNyAtMTY0IDEwIC00MCAyMSAtNzkgNjkgLTkyIDExNCAtNSAyMCAtMTAgMTAyIC0xMCAxODIgMCA4MCAtNSAxNjIgLTExIDE4NCAtMjIgNzkgLTEzNSAxNjYgLTIzNCAxODEgLTM3IDYgLTM1IDMgMzAgLTI4IDc4IC0zOSAxNDQgLTkxIDEzMiAtMTA0IC01IC00IC0zNyAtOCAtNzEgLTggLTc3IDAgLTExNyAyNCAtMTgyIDEwOSAtNTIgNjggLTUxIDcwIDQyIDg1IDcxIDExIDE0MyAwIDE4MyAtMjkgMTYgLTExIDQwIC00MyA1NCAtNzMgMTMgLTI5IDMyIC01OSA0MSAtNjYgMTQgLTEyIDE2IC03IDE2IDU4IDAgNTkgNCA3NyAyMyAxMDIgMTkgMjYgMjMgNDYgMjUgMTMwIDMgNjcgMCA5OSAtNyA5OSAtNyAwIC0xMSAtMjMgLTEyIC01NyAwIC0zMiAtNiAtNzYgLTEyIC05NyBsLTEyIC00MCAtMjcgMzIgYy0zNCA0MSAtNDMgOTYgLTI0IDE1MSAxNCA0MSA3NSAxNDEgODYgMTQxIDMgMCAyMSAtMjQgNDAgLTUyeiIvPjwvZz48L3N2Zz4K)](https://github.com/max-sixty/tend)

> **Noviembre de 2026**: Worktrunk se [publicó](https://x.com/max_sixty/status/2006077845391724739?s=20) a principios de año y enseguida se convirtió en el gestor de worktrees de git más popular. Está hecho con cariño (¡nada de slop!). Cuéntame cualquier fricción, por pequeña que sea; estoy muy centrado en seguir haciendo que Worktrunk sea excelente, y la mayor ayuda es que la gente publique los problemas que percibe.

Worktrunk es una CLI para gestionar worktrees de git, pensada para ejecutar agentes de IA en paralelo.

Los tres comandos principales de Worktrunk hacen que las worktrees sean tan fáciles como las ramas. Además, Worktrunk trae un buen puñado de comodidades para simplificar el trabajo con muchos cambios en paralelo, incluidos hooks para automatizar flujos locales y cachés de compilación con copy-on-write.

Una demostración rápida:

![Demostración de Worktrunk](https://cdn.jsdelivr.net/gh/max-sixty/worktrunk-assets@main/assets/docs/light/wt-core.gif)

> ### 📚 Documentación completa en [worktrunk.dev](https://worktrunk.dev) 📚

## Contexto: worktrees de git

Los agentes de IA como Claude Code y Codex pueden encargarse de tareas más
largas sin supervisión, hasta el punto de que es posible llevar 5, 10 o más en
paralelo. La función nativa de worktree de git da a cada agente su propio
directorio de trabajo, de modo que no se pisan los cambios entre ellos.

Pero la experiencia de uso de las worktrees de git es incómoda. Incluso una
tarea tan pequeña como iniciar una worktree nueva obliga a escribir el nombre de
la rama tres veces: `git worktree add -b feat ../repo.feat` y luego
`cd ../repo.feat`.

## Worktrunk hace que las worktrees de git sean tan fáciles como las ramas

Las worktrees se direccionan por el nombre de la rama; las rutas se calculan a partir de una plantilla configurable. Los comandos que reciben una rama también aceptan la ruta de la worktree en la que está activa.

<p class="workflow-stage">Empieza por los comandos principales</p>

**Comandos principales:**

<table class="cmd-compare">
  <thead>
    <tr>
      <th>Tarea</th>
      <th>Worktrunk</th>
      <th>Git a secas</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td>Cambiar de worktree</td>
      <td data-label="Worktrunk"><code>wt switch feat</code></td>
      <td data-label="Git a secas"><code>cd ../repo.feat</code></td>
    </tr>
    <tr>
      <td>Crear + arrancar Claude</td>
      <td data-label="Worktrunk"><code>wt switch -c -x claude feat</code></td>
      <td data-label="Git a secas"><pre><code>git worktree add -b feat ../repo.feat && \
cd ../repo.feat && \
claude</code></pre></td>
    </tr>
    <tr>
      <td>Limpiar</td>
      <td data-label="Worktrunk"><code>wt remove</code></td>
      <td data-label="Git a secas"><pre><code>cd ../repo && \
git worktree remove ../repo.feat && \
git branch -d feat</code></pre></td>
    </tr>
    <tr>
      <td>Listar con estado</td>
      <td data-label="Worktrunk"><code>wt list</code></td>
      <td data-label="Git a secas"><span class="cmd-compare-value"><code>git worktree list</code> (solo rutas)</span></td>
    </tr>
  </tbody>
</table>

<p class="workflow-stage">Pasa a los comandos más avanzados según los necesites</p>

<p class="workflow-heading"><strong>Automatización del flujo de trabajo:</strong></p>

- **[Hooks](https://worktrunk.dev/hook/)** — ejecuta comandos al crear, antes del merge, después del merge, etc.
- **[Mensajes de commit por LLM](https://worktrunk.dev/llm-commits/)** — genera mensajes de commit a partir de los diffs
- **[Flujo de merge](https://worktrunk.dev/merge/)** — squash, rebase, merge y limpieza en un solo comando
- **[Selector interactivo](https://worktrunk.dev/switch/#interactive-picker)** — recorre las worktrees con el estado de CI en directo y vistas previas de diff, log, PR y comentarios
- **[Cachés de compilación compartidas](https://worktrunk.dev/step/#wt-step-copy-ignored)** — diez worktrees obtienen `target/`, `node_modules/`, etc. sin compilarlos ni copiarlos (en APFS, btrfs y XFS)
- **[`wt list --full`](https://worktrunk.dev/list/#full-mode)** — [estado de CI](https://worktrunk.dev/list/#ci-status) y [resúmenes generados por IA](https://worktrunk.dev/list/#llm-summaries) por rama
- **[Checkout de PR](https://worktrunk.dev/switch/#pull-requests-and-merge-requests)** — `wt switch pr:123` para saltar directamente a la rama de un PR
- **[Servidor de desarrollo por worktree](https://worktrunk.dev/tips-patterns/#dev-server-per-worktree)** — el filtro de plantilla `hash_port` da a cada worktree un puerto único
- **[Alias](https://worktrunk.dev/extending/#aliases) y [variables por rama](https://worktrunk.dev/config/#wt-config-state-vars)** — comandos `wt <nombre>` personalizados y estado con ámbito de rama para las plantillas de hooks
- ...y **[mucho más](#próximos-pasos)**

Varios agentes en paralelo, los mismos comandos sencillos:

![Demostración completa de Worktrunk: varios agentes Claude en pestañas de Zellij con hooks, commits por LLM y flujo de merge](https://raw.githubusercontent.com/max-sixty/worktrunk-assets/main/assets/docs/light/wt-zellij-omnibus.gif)

## Instalación

**Homebrew (macOS y Linux):**

```bash
brew install worktrunk && wt config shell install
```

La integración con el shell permite que los comandos cambien de directorio.

**Cargo:**

```bash
cargo install worktrunk && wt config shell install
```

<details>
<summary><strong>Windows y otros</strong></summary>

**Windows.** `wt` es por defecto el comando de Windows Terminal, así que Winget instala además Worktrunk como `git-wt` para evitar el conflicto:

```bash
winget install max-sixty.worktrunk
git-wt config shell install
```

Como alternativa, desactiva el alias de Windows Terminal (Configuración → Aplicaciones → Configuración avanzada de aplicaciones → Alias de ejecución de aplicaciones → "Terminal"/"Terminal Preview") para usar `wt` directamente.

> Firma de código gratuita proporcionada por [SignPath.io](https://signpath.io/), certificado de [SignPath Foundation](https://signpath.org/) — [política](https://worktrunk.dev/code-signing/).

**Arch Linux:**

```bash
sudo pacman -S worktrunk && wt config shell install
```

**Conda / Pixi** ([feedstock](https://github.com/conda-forge/worktrunk-feedstock) mantenido por la comunidad):

```bash
conda install -c conda-forge worktrunk && wt config shell install
```

O con [Pixi](https://pixi.sh): `pixi global install worktrunk && wt config shell install`.

</details>

## Inicio rápido

Crea una worktree para una nueva funcionalidad:

```console
$ wt switch --create feature-auth
✓ Created branch feature-auth from main and worktree @ ~/repo.feature-auth
```

Esto crea una rama y una worktree nuevas y cambia a ella. Haz tu trabajo y después revisa todas las worktrees con [`wt list`](https://worktrunk.dev/list/):

```console
$ wt list
  Branch        Status      HEAD±     main↕    main…±    Remote⇅  Commit    Age  Message
@ feature-auth  +   ↑      +27   -8   ↑1       +31                4bc72dc    2h  Add authenticatio…
^ main              ^⇡                                    ⇡1      0e631ad    1d  Initial commit

○ Showing 2 worktrees, 1 with changes, 1 ahead, hidden: Path
```

La `@` marca la worktree actual. `+` indica cambios en el área de preparación, `↑1` indica 1 commit por delante de main y `⇡` indica commits sin subir.

Al terminar, elige entre:

**Flujo con PR** — commit, push, abrir un PR, hacer merge desde GitHub/GitLab y luego limpiar:

```bash
wt step commit                    # hace commit de los cambios preparados
gh pr create                      # o glab mr create
wt remove                         # después de que el PR se haya fusionado
```

**Merge local** — squash, rebase sobre main, merge fast-forward y limpieza:

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

Para agentes en paralelo, crea varias worktrees y lanza un agente en cada una:

```bash
wt switch -x claude -c feature-a -- 'Add user authentication'
wt switch -x claude -c feature-b -- 'Fix the pagination bug'
wt switch -x claude -c feature-c -- 'Write tests for the API'
```

La bandera `-x` ejecuta un comando después de cambiar; los argumentos que van tras `--` se le pasan a él. Configura [hooks de post-arranque](https://worktrunk.dev/hook/#hook-types) para automatizar la preparación (instalar dependencias, levantar servidores de desarrollo).

## Próximos pasos

- Aprende los comandos principales: [`wt switch`](https://worktrunk.dev/switch/), [`wt list`](https://worktrunk.dev/list/), [`wt merge`](https://worktrunk.dev/merge/), [`wt remove`](https://worktrunk.dev/remove/)
- Configura [hooks](https://worktrunk.dev/hook/) para preparar el entorno automáticamente
- Explora los [mensajes de commit por LLM](https://worktrunk.dev/llm-commits/), el [selector
  interactivo](https://worktrunk.dev/switch/#interactive-picker), la [integración con Claude Code](https://worktrunk.dev/claude-code/) y el [estado de CI
  y los enlaces a PR](https://worktrunk.dev/list/#ci-status)
- Echa un vistazo a [consejos y patrones](https://worktrunk.dev/tips-patterns/) para ver recetas: alias, servidores de desarrollo, bases de datos, relevos entre agentes y más
- [Extender Worktrunk](https://worktrunk.dev/extending/) — personaliza los flujos con hooks y alias
- Mira el [vídeo de @DevOpsToolbox sobre Worktrunk](https://youtu.be/WBQiqr6LevQ?t=345)
- Ejecuta `wt --help` o `wt <comando> --help` para una referencia rápida de la CLI

## Cómo contribuir

- ⭐ Dale una estrella al repositorio
- Háblale a alguien de Worktrunk
- [Abre una issue](https://github.com/max-sixty/worktrunk/issues/new?title=&body=%23%23%20Description%0A%0A%3C!--%20Describe%20the%20bug%20or%20feature%20request%20--%3E%0A%0A%23%23%20Context%0A%0A%3C!--%20Any%20relevant%20context%3A%20your%20workflow%2C%20what%20you%20were%20trying%20to%20do%2C%20etc.%20--%3E) — comentarios, peticiones de funcionalidades, incluso una pequeña fricción o un mensaje de usuario imperfecto, o [una molestia con las worktrees aún sin resolver](https://github.com/max-sixty/worktrunk/issues/new?title=Worktree%20friction%3A%20&body=%23%23%20The%20friction%0A%0A%3C!--%20What%20worktree-related%20task%20is%20still%20painful%3F%20--%3E%0A%0A%23%23%20Current%20workaround%0A%0A%3C!--%20How%20do%20you%20handle%20this%20today%3F%20--%3E%0A%0A%23%23%20Ideal%20solution%0A%0A%3C!--%20What%20would%20make%20this%20easier%3F%20--%3E)
- Comparte: [X](https://twitter.com/intent/tweet?text=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management&url=https%3A%2F%2Fworktrunk.dev) · [Reddit](https://www.reddit.com/submit?url=https%3A%2F%2Fworktrunk.dev&title=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management) · [LinkedIn](https://www.linkedin.com/sharing/share-offsite/?url=https%3A%2F%2Fworktrunk.dev)

> ### 📚 Documentación completa en [worktrunk.dev](https://worktrunk.dev) 📚

### Historial de estrellas

<a href="https://star-history.com/#max-sixty/worktrunk&Date">
  <img src="https://api.star-history.com/svg?repos=max-sixty/worktrunk&type=Date&sealed_token=2ySbQiVbkVrGmwgDsJya-xr4ApbVVvR0siYI46d22Xj_1kPCcgA9X0YpUGc3__aMuZ0ZAWzG4NBhJtqepYjlkoYrVwmKbgaPmGpNZTCfSyVp8EDA_IXaOOYW2whsOXDAi6g7HD9ezsnqSl58n7AqW2_4IQ4hY2p3h7tNv_3k4am5ASq1NdpbfrOUazCl" width="500" alt="Star History Chart">
</a>
