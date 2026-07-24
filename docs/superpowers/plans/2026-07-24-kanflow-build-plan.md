# kanflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bootstrap and build the kanflow project using codecoder's workgraph + headless BG + cc interactive drive, covering full-stack Kanban (Rust API + React SPA + SSE real-time + multi-user auth + drag-and-drop).

**Architecture:** Single Rust crate (axum + sqlx/SQLite + argon2 + tokio) serves REST API + SSE, hosts `web/dist` static in production. React/Vite/TS SPA in `web/`. All acceptance gates are bare `make X` commands so bg_gate matches them as Command gate.

**Tech Stack:** Rust (axum, sqlx, tokio, argon2, serde), React (Vite, TypeScript, vitest), SQLite, SSE, fractional-index (generateKeyBetween).

## Global Constraints

- sqlx: use runtime `sqlx::query` / `query_as` only — NO compile-time macros (no `sqlx::query!` / `query_as!`)
- Position field: always TEXT, use fractional-index string keys (not integer sequential)
- Error: unify as `AppError` implementing `IntoResponse`; all handlers return `Result<Json<T>, AppError>`
- Every board-scoped handler must verify: the authenticated user is a member of that board
- Never run two concurrent cc messages to the same daemon (shared session history file race)
- All milestone acceptance gates are bare `make X` commands (extract_gate_command sees `make ` pattern + ASCII)

**User's memory reminder — how to drive codecoder headless:**
- `codecoder.json` allowlist keys: `write_file, edit_file, commit, generate_skill, generate_capability, run_capability, run_command:cargo, run_command:make, run_command:npm, run_command:sqlx, run_command:git`
- `CODECODER_DEFAULT_TRUST=always` for headless (env var per run)
- SECRETS (API_KEY/API_BASE) must come from real shell `export`, NOT from `.ccd.env`
- Never send concurrent cc messages; serialize and wait for each turn
- bg_gate uses `extract_gate_command` from acceptance text: `make api-test` → matches `make ` pattern + ASCII → runs command gate

---

## File Structure

**kanflow/** (to be created):

```
kanflow/
  AGENTS.md                     # project identity + dev discipline + no-over-exploration
  CONTEXT.md                    # glossary of domain terms
  codecoder.json                # allowlist
  workgraph.json                # milestone dependency graph
  .ccd.env                      # CODECODER_* env vars (max_tokens, model, BG_*)
  Cargo.toml                    # crate config
  migrations/
    001_initial.sql             # schema
  Makefile                      # make targets for gates
  src/
    main.rs                     # axum server bootstrap (router layers, static files)
    lib.rs                      # crate root
    db.rs                       # sqlx::Executor helpers (runtime mode)
    auth.rs                     # argon2 hash/verify, session CRUD, auth middleware
    models.rs                   # struct defs (User, Board, List, Card, Label, Session, Activity)
    handlers/
      mod.rs
      auth_handler.rs           # register/login/logout/me
      board_handler.rs          # board CRUD + members
      list_handler.rs           # list CRUD + move
      card_handler.rs           # card CRUD + cross-list move
      label_handler.rs          # label CRUD + card→label attach/detach
      sse_handler.rs            # SSE event stream (GET /api/boards/:id/events)
    ordering.rs                 # fractional-index key_between
    sse.rs                      # broadcast hub (tokio::sync::broadcast per board)
    error.rs                    # AppError
  tests/
    integration_test.rs         # tower::oneshot tests (in-mem SQLite, full API exercise)
  web/
    index.html
    vite.config.ts
    tsconfig.json
    package.json
    src/
      main.tsx
      App.tsx
      api.ts                    # fetch helpers (login/logout/board CRUD/card CRUD/drag)
      types.ts                  # shared TS types
      auth/
        LoginPage.tsx
        RegisterPage.tsx
        AuthContext.tsx
      board/
        BoardListPage.tsx
        BoardPage.tsx
        ListColumn.tsx
        CardItem.tsx
        CardModal.tsx
        MemberInviteModal.tsx
        drag.ts                 # pointer event drag logic
        sse.ts                  # EventSource connection + merge
        filter.ts               # label/text filter helpers
      design/
        tokens.css              # design system tokens
        base.css                # reset, global styles
        LoginPage.css | ...     # per-component CSS (or inline)
      tests/
        (vitest unit tests)
```

---

### Task 1: User prepares control plane and trust

**Files:**
- Modify: `~/.codecoder/trust.json` — add kanflow trust entry
- Create: `~/Code/kanflow/codecoder.json` — allowlist
- Create: `~/Code/kanflow/.ccd.env` — env config
- Create: `~/Code/kanflow/AGENTS.md` — project identity
- Create: `~/Code/kanflow/CONTEXT.md` — domain glossary

**Interfaces:**
- Consumes: codecoder binary compilation, trust.json format, allowlist key format
- Produces: files that codecoder reads at startup

- [ ] **Step 1: Verify codecoder binary exists and compile if needed**

```bash
cd ~/Code/codecoder
if [ ! -f target/release/codecoder ]; then cargo build --release; fi
ls -la target/release/{codecoder,cc}
```

- [ ] **Step 2: Create kanflow directory, init git, copy binaries**

```bash
mkdir -p ~/Code/kanflow && cd ~/Code/kanflow && git init
cp ~/Code/codecoder/target/release/{codecoder,cc} bin/
echo "bin/" >> .gitignore
git add .gitignore && git commit -m "chore: init kanflow project"
```

- [ ] **Step 3: Write codecoder.json (allowlist)**

```json
{
  "allowlist": [
    "write_file",
    "edit_file",
    "commit",
    "generate_skill",
    "generate_capability",
    "run_capability",
    "run_command:cargo",
    "run_command:make",
    "run_command:npm",
    "run_command:sqlx",
    "run_command:git"
  ]
}
```

Write to `~/Code/kanflow/codecoder.json`.

- [ ] **Step 4: Write .ccd.env**

```
CODECODER_MODEL=deepseek-v4-flash
CODECODER_MAX_TOKENS=8192
CODECODER_BG_MAX_FIX_ATTEMPTS=3
CODECODER_BG_MAX_AUTO=10
CODECODER_BG_CIRCUIT_K=3
CODECODER_BG_MILESTONE_TOOL_CAP=12
```

Write to `~/Code/kanflow/.ccd.env`.

- [ ] **Step 5: Write AGENTS.md**

```markdown
# kanflow — Build Agent Identity

You are building the kanflow project: a multi-user real-time Kanban board.

**Tech stack:**
- Rust: axum server + sqlx (SQLite) + argon2 + tokio
- React + Vite + TypeScript SPA (in `web/`)

**You MUST follow these rules:**
1. sqlx: use runtime `sqlx::query("SELECT ...")` / `sqlx::query_as::<T,_>("...")` — NO compile-time macros (`query!` / `query_as!`)
2. Position field: always TEXT, use fractional-index string keys
3. Error: unify as `AppError` implementing `IntoResponse`; handlers return `Result<Json<T>, AppError>`
4. Every board-scoped handler must verify the authenticated user is a member of that board
5. DO NOT over-explore — write code, read what you need, and move forward
6. ALL acceptance gates are bare `make X` commands — ensure Makefile targets are exact
7. NEVER run two concurrent cc messages to the same daemon (file race)
8. Use `search_web` when needing reference implementations (axum SSE, fractional-index key generation)
```

Write to `~/Code/kanflow/AGENTS.md`.

- [ ] **Step 6: Write CONTEXT.md**

```markdown
# kanflow Glossary

- **Board**: a top-level kanban board, owned by a user, can have members
- **List**: a column within a board (e.g., "To Do", "In Progress")
- **Card**: a task card within a list, can be dragged between lists
- **Position**: a fractional-index TEXT field for ordering (compact midpoint string, max 64 chars)
- **Fractional-index**: algorithm that generates a sortable key between two existing keys (like generateKeyBetween(a,b))
- **SSE hub**: a tokio::sync::broadcast channel per board ID, broadcasting JSON event payloads
- **Event**: a JSON payload sent via SSE on every board mutation (create/update/delete list/card/member)
- **Last-Event-ID**: SSE resumption — client sends this query param on reconnect, server replays missed events
- **AppError**: the unified error type implementing `IntoResponse`, returning JSON `{"error":"..."}`
- **Auth middleware**: tower::Service extracting session cookie, populating request extensions with user_id
- **Board membership guard**: every board-scoped handler checks `board_members` table for the current user

_Avoid_: "column" (use "list"), "fractional indexing" (use "fractional-index"), "task" (use "card"), "workspace" (use "board")
```

Write to `~/Code/kanflow/CONTEXT.md`.

- [ ] **Step 7: Add kanflow to trust.json**

```bash
cd ~/Code/kanflow && ls ~/.codecoder/trust.json && cat ~/.codecoder/trust.json | python3 -c "
import json,sys
t=json.load(sys.stdin)
t['decisions'][r'$HOME/Code/kanflow']='trusted'
json.dump(t,sys.stdout,indent=2)
" > /tmp/trust_new.json && cp /tmp/trust_new.json ~/.codecoder/trust.json
```

(Alternatively, just add the line manually — this is a one-time setup step.)

- [ ] **Step 8: Commit control plane files**

```bash
cd ~/Code/kanflow
git add codecoder.json .ccd.env AGENTS.md CONTEXT.md
git commit -m "chore: add codecoder control plane files"
```

---

### Task 2: Create workgraph.json (milestone DAG)

**Files:**
- Create: `~/Code/kanflow/workgraph.json`

**Interfaces:**
- Consumes: workgraph.json format (Milestone: id, title, acceptance, deps, status, command)
- Produces: the DAG that codecoder's headless BG_WORKGRAPH reads

- [ ] **Step 1: Write workgraph.json**

```json
{
  "schema_version": 1,
  "nodes": [
    {
      "id": 1,
      "title": "M0: Scaffold project (Cargo + Makefile + web/ skeleton + migrations/)",
      "acceptance": "make scaffold-check",
      "deps": [],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    },
    {
      "id": 2,
      "title": "M1: Database schema (6+ tables) + db module + models (runtime sqlx::query)",
      "acceptance": "make api-test",
      "deps": [1],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    },
    {
      "id": 3,
      "title": "M2: Auth (register/login/logout/me) + argon2id + sessions + auth middleware",
      "acceptance": "make api-test",
      "deps": [1],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    },
    {
      "id": 4,
      "title": "M3: Fractional-index ordering module (key_between) + unit tests",
      "acceptance": "make api-test",
      "deps": [1],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    },
    {
      "id": 5,
      "title": "M4: Board/List/Card CRUD handlers + member guard + move via M3 ordering + activity stream",
      "acceptance": "make api-test",
      "deps": [2, 3, 4],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    },
    {
      "id": 6,
      "title": "M5: SSE hub (tokio broadcast per board) + GET /api/boards/:id/events + broadcast on mutation",
      "acceptance": "make api-test",
      "deps": [5],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    },
    {
      "id": 7,
      "title": "M6: Frontend design tokens + base components + login/register pages wired to API + session state",
      "acceptance": "make web-check",
      "deps": [1],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    },
    {
      "id": 8,
      "title": "M7: Board list page + board detail (columns + cards) + card modal + labels/members UI",
      "acceptance": "make web-check",
      "deps": [7],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    },
    {
      "id": 9,
      "title": "M8: Pointer drag-and-drop (cards cross-list, within-list, lists reorder) + optimistic update + SSE merge + filters + keyboard shortcuts",
      "acceptance": "make web-check",
      "deps": [8],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    },
    {
      "id": 10,
      "title": "M9: Axum serves web/dist static + smoke test capability + full check pass",
      "acceptance": "make check",
      "deps": [6, 9],
      "status": "pending",
      "touched": [],
      "fix_attempts": 0
    }
  ]
}
```

Write to `~/Code/kanflow/workgraph.json`.

- [ ] **Step 2: Initialize Makefile with placeholder targets**

```makefile
SHELL := /bin/bash
ROOT := $(shell pwd)

.PHONY: scaffold-check api-test web-check check

scaffold-check:
	cargo build && cd web && npm ci && npx tsc --noEmit && npx vite build

api-test:
	cargo test

web-check:
	cd web && npx tsc --noEmit && npx vitest run && npx vite build

check:
	$(MAKE) api-test && $(MAKE) web-check
```

Write to `~/Code/kanflow/Makefile`.

- [ ] **Step 3: Commit workgraph + Makefile**

```bash
cd ~/Code/kanflow
git add workgraph.json Makefile
git commit -m "chore: add workgraph DAG and Makefile targets"
```

---

### Task 3: Interactive cc pass — M0 (Scaffold)

**This task runs interactively with cc (not headless BG).**

**Files to create (by codecoder):**
- `~/Code/kanflow/Cargo.toml`
- `~/Code/kanflow/src/main.rs` (binary entrypoint: axum health check only)
- `~/Code/kanflow/src/lib.rs` (empty crate root)
- `~/Code/kanflow/migrations/001_initial.sql` (placeholder)
- `~/Code/kanflow/web/package.json`, `web/vite.config.ts`, `web/tsconfig.json`, `web/index.html`
- `~/Code/kanflow/web/src/main.tsx` (hello world React)

- [ ] **Step 1: Start ccd daemon**

```bash
cd ~/Code/kanflow
# Export secrets first (they won't be loaded from .ccd.env):
export CODECODER_API_KEY="sk-b8a71250b86e45a5967ebe24631f4993"
export CODECODER_API_BASE="https://api.deepseek.com"
export CODECODER_DEFAULT_TRUST=always
CODECODER_DAEMON=1 cargo run &
sleep 3
echo "daemon started (PID $!)"
```

- [ ] **Step 2: Send cc command for M0 scaffold**

```bash
cd ~/Code/kanflow
./bin/cc "M0: Scaffold the kanflow project exactly as instructed.

Create these files:
1. Cargo.toml — binary crate named 'kanflow-server', dependencies: axum, tokio (full features), sqlx (runtime-tokio-rustls, sqlite), argon2, serde/serde_json, uuid (v4), tower-http (cors), tower, anyhow
2. src/main.rs — minimal axum server on port 3001 with a single GET /api/health → 'ok'
3. src/lib.rs — empty
4. migrations/001_initial.sql — placeholder: CREATE TABLE IF NOT EXISTS _schema_version (version INTEGER);
5. web/package.json — React 18 + Vite 5 + TypeScript + vitest; scripts: dev/build/preview/test
6. web/vite.config.ts — basic Vite config listening on port 5173, proxy /api to http://localhost:3001
7. web/tsconfig.json — strict TS
8. web/index.html — mount #root
9. web/src/main.tsx — renders <h1>kanflow</h1> into #root

IMPORTANT rules:
- DO NOT use sqlx compile-time macros (!)
- Keep it minimal — we only need cargo build and vite build to pass
- After creating all files, run 'cargo build' and 'cd web && npm install'

End your reply with EXACTLY: VERDICT: pass"
```

- [ ] **Step 3: Verify M0 manually**

```bash
cd ~/Code/kanflow
make scaffold-check 2>&1 | head -30
```

If it fails, analyze output and send a targeted fix via cc. If it passes, continue.

- [ ] **Step 4: Kill daemon**

```bash
kill %1 2>/dev/null; sleep 1
```

- [ ] **Step 5: Commit M0 (done by codecoder, verify)**

```bash
cd ~/Code/kanflow
git status --short
git add -A && git commit -m "feat: M0 scaffold — cargo + vite + react skeleton"
```

---

### Task 4: Headless BG_WORKGRAPH — M1 (DB + migrations)

**This task runs headless via BG_WORKGRAPH.**

- [ ] **Step 1: Start headless BG run targeting workgraph**

```bash
cd ~/Code/kanflow
export CODECODER_API_KEY="sk-b8a71250b86e45a5967ebe24631f4993"
export CODECODER_API_BASE="https://api.deepseek.com"
export CODECODER_DEFAULT_TRUST=always
export CODECODER_BG_WORKGRAPH=1
# codecoder auto-loads .ccd.env for CODECODER_ prefix vars (but not API_KEY etc.)
cargo run 2>&1
echo "Exit code: $?"
```

- [ ] **Step 2: Check exit code and workgraph state**

```bash
cd ~/Code/kanflow
cat workgraph.json | python3 -m json.tool | grep '"status"' | sort
ls src/db.rs src/models.rs migrations/001_initial.sql 2>/dev/null
```

Exit 0 (CompletedAllReady) = M1 passed. Exit 2 (StuckNeedsFix) = needs_fix with no retry budget.

- [ ] **Step 3: If stuck, read failure and send fix via cc (interactive)**

```bash
cd ~/Code/kanflow
cat workgraph.json | python3 -c "import json,sys; n=[n for n in json.load(sys.stdin)['nodes'] if n['id']==2][0]; print('fix_attempts:', n['fix_attempts']); print('last_failure:', n.get('last_failure',''))"
```

If stuck: start daemon, send cc with targeted fix based on failure, re-check.

- [ ] **Step 4: Verify M1 independently**

```bash
cd ~/Code/kanflow
make api-test 2>&1 | tail -5
```

- [ ] **Step 5: Wait for headless to auto-progress to M2**

If M1 passed and M2 is ready, headless continues automatically. Monitor with:

```bash
cd ~/Code/kanflow
while true; do python3 -c "
import json
g=json.load(open('workgraph.json'))
for n in g['nodes']:
  print(f'#{n[\"id\"]}: {n[\"status\"]}')
"; echo "---"; sleep 5; done
```

---

### Task 5: Headless BG_WORKGRAPH — M2 (Auth) + M3 (Ordering) + M4 (CRUD) chain

- [ ] **Step 1: Start headless run (if not already running)**

Same as Task 4 Step 1. Headless auto-advances through ready milestones.

- [ ] **Step 2: Monitor progress — wait for M4 to complete**

Exit 0 = all M2→M4 done. Exit 2/3 = stuck somewhere.

- [ ] **Step 3: If stuck at any node, diagnose and fix**

Read the milestone's `last_failure` and `fix_attempts`. If budget exhausted (`fix_attempts` == `BG_MAX_FIX_ATTEMPTS`), manually reset to `pending` and re-run with a more precise task instruction:

```bash
cd ~/Code/kanflow
# Manually reset a stuck milestone:
python3 -c "
import json
g=json.load(open('workgraph.json'))
for n in g['nodes']:
  if n['id'] == TARGET_ID:
    n['status'] = 'pending'
    n['fix_attempts'] = 0
    n['last_failure'] = None
json.dump(g, open('workgraph.json','w'), indent=2)
"
```

Then start headless with explicit task:
```bash
cd ~/Code/kanflow
export CODECODER_BG_TASK="M2: Implement auth. Create src/auth.rs with:
- hash_password(password: &str) -> String (using argon2)
- verify_password(hash: &str, password: &str) -> bool
- create_session(pool, user_id) -> String (random token, store in sessions table)
- get_user_by_session(pool, token) -> Option<User>
- AuthMiddleware: axum middleware that reads 'session' cookie, looks up user, sets extension

Create src/handlers/auth_handler.rs with:
- POST /api/register { username, password } -> 201 { user }
- POST /api/login { username, password } -> 200 { user } (sets Set-Cookie)
- POST /api/logout -> 200 (clears cookie + deletes session)
- GET /api/me -> 200 { user }
- GET /api/me without cookie -> 401

Register these routes in main.rs under Router::new().nest(\"/api\", ...)

IMPORTANT: sqlx runtime mode only. Return AppError for errors. After impl, cargo test must pass.

VERDICT: pass"

cargo run 2>&1
```

- [ ] **Step 4: Verify M2-M4 independently after completion**

```bash
cd ~/Code/kanflow
make api-test 2>&1 | tail -10
```

- [ ] **Step 5: Commit all M2-M4 work**

```bash
cd ~/Code/kanflow
git add -A && git commit -m "feat: M2-M4 — auth + ordering + CRUD handlers"
```

---

### Task 6: Headless — M5 (SSE hub) (depends on M4)

- [ ] **Step 1: Start headless run for M5**

```bash
cd ~/Code/kanflow
export CODECODER_BG_WORKGRAPH=1
# (env already exported from previous step if same shell)
cargo run 2>&1
```

- [ ] **Step 2: Monitor and handle stuck nodes same as Task 5**

M5 implements:
- `src/sse.rs` — per-board broadcast channels (HashMap<board_id, broadcast::Sender<Event>>), lazy creation
- `src/handlers/sse_handler.rs` — GET /api/boards/:id/events: SSE stream, reads session cookie, verifies board membership, subscribes to broadcast
- Add broadcast calls in every board/list/card handler after successful mutation
- Event JSON: `{ "type": "card_created"|"card_moved"|"list_created"|"list_moved"|"board_member_added"|..., "payload": {...}, "user_id": ..., "created_at": ... }`
- Support `Last-Event-ID` query param: if present, replay events since that timestamp (store recent events in a VecDeque in the sse module)

- [ ] **Step 3: Verify M5 independently**

```bash
cd ~/Code/kanflow
make api-test 2>&1 | tail -10
```

- [ ] **Step 4: Commit M5**

```bash
cd ~/Code/kanflow
git add -A && git commit -m "feat: M5 — SSE hub + broadcast on mutation"
```

---

### Task 7: Interactive cc — M6 (Frontend design tokens + auth pages)

**Switch to interactive cc for frontend work — codecoder's deepseek-v4-flash writes TS/React more reliably with precise cc prompts.**

- [ ] **Step 1: Start daemon**

```bash
cd ~/Code/kanflow
export CODECODER_API_KEY="sk-b8a71250b86e45a5967ebe24631f4993"
export CODECODER_API_BASE="https://api.deepseek.com"
export CODECODER_DEFAULT_TRUST=always
CODECODER_DAEMON=1 cargo run &
sleep 3
```

- [ ] **Step 2: Send cc command for M6 (design tokens + auth pages)**

```bash
cd ~/Code/kanflow
./bin/cc "M6: Frontend design tokens + auth pages + session state.

Create web/src/design/tokens.css with CSS custom properties:
--color-primary: #6366f1;  /* indigo accent */
--color-bg: #f8fafc;
--color-surface: #ffffff;
--color-text: #1e293b;
--color-text-secondary: #64748b;
--color-border: #e2e8f0;
--color-danger: #ef4444;
--radius-sm: 6px;
--radius-md: 10px;
--radius-lg: 14px;
--space-xs: 4px; --space-sm: 8px; --space-md: 16px; --space-lg: 24px; --space-xl: 32px;
--font-sans: 'Inter', system-ui, sans-serif;
--shadow-sm: 0 1px 3px rgba(0,0,0,0.08);
--shadow-md: 0 4px 12px rgba(0,0,0,0.1);

Create web/src/design/base.css: reset, import tokens, body styles.

Create web/src/types.ts:
export interface User { id: number; username: string; }
export interface Board { id: number; name: string; owner_id: number; }
export interface List { id: number; board_id: number; name: string; position: string; }
export interface Card { id: number; list_id: number; title: string; description: string; position: string; }
export interface Label { id: number; board_id: number; name: string; color: string; }
export interface Activity { id: number; board_id: number; user_id: number; kind: string; payload: any; created_at: string; }

Create web/src/api.ts with fetch wrappers (+credentials: 'include'):
- async function api<T>(path, options?) -> T — base fetch with error handling
- login(username, password) -> User | throw
- register(username, password) -> User | throw
- logout() -> void
- getMe() -> User | null (returns null on 401)
- createBoard(name) -> Board
- ... (more will be added in M7, keep minimal now)

Create web/src/auth/AuthContext.tsx: React context providing:
- user: User | null (null = loading, check on mount via getMe())
- login, register, logout functions (update state after success)
- loading: boolean

Create web/src/auth/LoginPage.tsx:
- Clean centered card with logo/heading, username input, password input, submit button
- Calls login() on submit, redirects to /boards on success
- Link to register page
- Minimal styling with tokens.css

Create web/src/auth/RegisterPage.tsx:
- Same layout as login, username+password+confirm, link back to login

Create web/src/App.tsx with React Router:
- / -> redirect to /boards if logged in, else /login
- /login -> LoginPage
- /register -> RegisterPage
- Wrap with AuthContext provider
- Loading spinner while auth state resolves

Update web/src/main.tsx to render App.tsx.

After all files: cd web && npx tsc --noEmit must pass.
VERDICT: pass"
```

- [ ] **Step 3: Verify M6**

```bash
cd ~/Code/kanflow
make web-check 2>&1 | tail -20
```

If tsc errors, send targeted cc fix. If passes, kill daemon and commit.

- [ ] **Step 4: Kill daemon and commit**

```bash
kill %1 2>/dev/null; sleep 1
cd ~/Code/kanflow
git add -A && git commit -m "feat: M6 — design tokens + auth pages"
```

---

### Task 8: Interactive cc — M7 (Board views + card modal + labels)

- [ ] **Step 1: Start daemon**

```bash
cd ~/Code/kanflow
export CODECODER_API_KEY="sk-b8a71250b86e45a5967ebe24631f4993"
export CODECODER_API_BASE="https://api.deepseek.com"
export CODECODER_DEFAULT_TRUST=always
CODECODER_DAEMON=1 cargo run &
sleep 3
```

- [ ] **Step 2: Send cc command for M7**

```bash
cd ~/Code/kanflow
./bin/cc "M7: Board views, card modal, labels/members UI.

Add to web/src/api.ts:
- getBoards() -> Board[]
- getBoard(id) -> Board
- getBoardLists(boardId) -> List[]
- getBoardCards(boardId) -> Card[]
- getBoardLabels(boardId) -> Label[]
- createList(boardId, name) -> List
- createCard(listId, title) -> Card
- deleteCard(id) -> void
- addBoardMember(boardId, username) -> void
- attachLabel(cardId, labelId) -> void
- detachLabel(cardId, labelId) -> void
- getActivity(boardId) -> Activity[]

Create web/src/board/BoardListPage.tsx:
- Displays all boards as cards in a grid
- 'Create Board' button that opens inline form
- Each board card links to /boards/:id
- Logout button

Create web/src/board/BoardPage.tsx:
- Horizontal scroll container with ListColumn components
- Board header with name, member invite button
- Labels bar below header
- SSE EventSource connection (web/src/board/sse.ts) for real-time updates
- Accepts board ID from URL params

Create web/src/board/ListColumn.tsx:
- Vertical card container with column header (name + card count)
- Add Card button at bottom
- Displays CardItem components
- Styled as a column card (light surface, rounded, shadow)

Create web/src/board/CardItem.tsx:
- Card with title, label dots, click handler
- Simple hover effect

Create web/src/board/CardModal.tsx:
- Modal overlay with card title (editable textarea), description (textarea)
- Labels section: show attached labels with remove, add label dropdown
- Delete card button
- Esc closes, click outside closes
- Enter on title saves

Create web/src/board/MemberInviteModal.tsx:
- Input for username, Send button
- Displays current members

IMPORTANT: Use tokens.css variables for all styling. Keep components clean and well-structured.

After: npx tsc --noEmit && npx vitest run must pass.
VERDICT: pass"
```

- [ ] **Step 3: Verify M7**

```bash
cd ~/Code/kanflow
make web-check 2>&1 | tail -20
```

- [ ] **Step 4: Commit**

```bash
kill %1 2>/dev/null; sleep 1
cd ~/Code/kanflow
git add -A && git commit -m "feat: M7 — board views + card modal + labels"
```

---

### Task 9: Interactive cc — M8 (Drag-and-drop + optimistic + SSE merge + filter)

**The hardest milestone — frontend drag-and-drop with pointer events.**

- [ ] **Step 1: Start daemon**

```bash
cd ~/Code/kanflow
export CODECODER_API_KEY="sk-b8a71250b86e45a5967ebe24631f4993"
export CODECODER_API_BASE="https://api.deepseek.com"
export CODECODER_DEFAULT_TRUST=always
CODECODER_DAEMON=1 cargo run &
sleep 3
```

- [ ] **Step 2: Send cc for drag-and-drop module**

```bash
cd ~/Code/kanflow
./bin/cc "M8 part 1: Create web/src/board/drag.ts — pointer-based drag-and-drop for kanban cards and lists.

Key constraints:
1. Card drag: pointerdown on card starts drag; moves the card element to follow pointer; on pointerup, determine which list the card is over and at what position within that list
2. Cross-list drag: when card is dropped over a different list, call API to move card: PATCH /api/cards/:id { list_id: targetListId, position: newPosition }
3. Within-list reorder: same API endpoint, just same list_id with new position
4. List reorder: drag the list column header; pointerdown on column header; call PATCH /api/lists/:id { position: newPosition }
5. Use pointer events (pointerdown, pointermove, pointerup), NOT HTML5 drag API
6. Clone card element for drag ghost; original stays in place
7. After successful API call, update local state with server response

Function signatures to export as a module:
- makeCardDraggable(cardEl: HTMLElement, cardData: { id, listId }, onDrop: (cardId, targetListId, position) => Promise<void>): void
- makeListDraggable(headerEl: HTMLElement, listData: { id }, onDrop: (listId, position) => Promise<void>): void
- fractionalIndex(a: string | null, b: string | null): string — generates position between a and b (use generateKeyBetween logic)

The drag module should be pure DOM — no React state management inside it. It fires callbacks that React components handle.

IMPORTANT: Use pointer capture. Prevent text selection during drag.

After creating drag.ts: npx tsc --noEmit must pass.
VERDICT: pass"
```

- [ ] **Step 3: Verify drag module compiles**

```bash
cd ~/Code/kanflow
make web-check 2>&1 | tail -5
```

- [ ] **Step 4: Send cc for optimistic update + SSE merge + filter + keyboard**

```bash
cd ~/Code/kanflow
./bin/cc "M8 part 2: Integrate drag into BoardPage + optimistic state + SSE merge + filter + keyboard.

Update web/src/board/sse.ts:
function connectSSE(boardId: number, onEvent: (event) => void): EventSource {
  // Connect to /api/boards/{boardId}/events with credentials: 'include'
  // Parse SSE data as JSON, call onEvent
  // Return the EventSource so caller can close()
}

Update web/src/board/BoardPage.tsx:
- State: lists with nested cards, each with a position field
- On mount: fetch lists + cards + labels from API
- On each list: use makeListDraggable on the column header
- On each card: use makeCardDraggable on the card element
- On card drop: optimistically move card in local state, call PATCH /api/cards/:id, on server response correct position
- On SSE event: merge event into local state (add/update/remove card/list)
- Add a filter bar: text input filters cards by title; label tag pills filter by label
- Keyboard shortcuts: Esc closes card modal, Enter in 'new card' input creates card, arrow keys move between cards when modal is open (if supported by browser)

Update web/src/board/CardItem.tsx and ListColumn.tsx to accept drag refs from parent:
- CardItem receives onPointerDown handler (from drag module)
- ListColumn receives onPointerDown for header drag

Important: When dragging, the card position calculation matters. Use:
- getBoundingClientRect() of each list column/card to determine drop target
- insertBefore / insertAfter in the DOM (visual indicator), then compute fractional-index position

After: npx tsc --noEmit must pass. npx vitest run must pass.
VERDICT: pass"
```

- [ ] **Step 5: Verify frontend build**

```bash
cd ~/Code/kanflow
make web-check 2>&1 | tail -20
```

- [ ] **Step 6: Commit M8**

```bash
kill %1 2>/dev/null; sleep 1
cd ~/Code/kanflow
git add -A && git commit -m "feat: M8 — drag-and-drop + optimistic + SSE merge + filter + keyboard"
```

---

### Task 10: Headless BG — M9 (Static serve + smoke test + final check)

- [ ] **Step 1: Start headless run for M9**

```bash
cd ~/Code/kanflow
export CODECODER_BG_WORKGRAPH=1
cargo run 2>&1
```

M9 should:
- Update `src/main.rs` to serve `web/dist` static files via tower-http::ServeDir at `/`
- Create a smoke capability: `capabilities/smoke/manifest.toml` with Shell/OnDemand lifecycle, that runs `curl` to test register→login→create board→create list→create card sequence
- `make check` must pass (both `cargo test` and `cd web && tsc --noEmit && vitest run && vite build`)

- [ ] **Step 2: Verify M9**

```bash
cd ~/Code/kanflow
make check 2>&1 | tail -20
echo "Exit: $?"
```

- [ ] **Step 3: Commit M9**

```bash
cd ~/Code/kanflow
git add -A && git commit -m "feat: M9 — static serve + smoke capability + final check pass"
```

---

### Task 11: Final verification — agent-browser visual check

- [ ] **Step 1: Start the server in background**

```bash
cd ~/Code/kanflow
# Ensure web/dist exists
(cd web && npx vite build 2>&1 | tail -3)
export CODECODER_API_KEY="sk-b8a71250b86e45a5967ebe24631f4993"
export CODECODER_API_BASE="https://api.deepseek.com"
cargo run &
sleep 3
echo "Server running on http://localhost:3001"
```

- [ ] **Step 2: Use agent-browser to verify the running app**

Open `http://localhost:3001/` (axum serves web/dist at root). Verify:
1. Landing page loads (should redirect to /login or show login page)
2. Register a user
3. Login with that user
4. See boards page (empty with 'Create Board' button)
5. Create a board
6. See board detail page (empty with 'Add List' option)
7. Create lists
8. Create cards in lists
9. Drag a card to another list
10. Open card modal, edit title/description, attach/detach label
11. Open second browser tab, verify SSE real-time updates

- [ ] **Step 3: Report visual/functional issues**

If any feature doesn't work visually (CSS issues, layout broken, drag not working, API errors), file them as issues and fix selectively.

- [ ] **Step 4: Kill server, final commit**

```bash
kill %1 2>/dev/null
cd ~/Code/kanflow
git add -A && git commit -m "fix: final polish after browser verification"
```

---

### Rollback / Stuck Recovery Strategy

| Situation | Action |
|---|---|
| Headless exits 2 (StuckNeedsFix) | Read `last_failure` from workgraph.json, reset node to `pending`, start daemon and send targeted cc fix |
| Headless exits 3 (CircuitBreaker) | Reset both failed nodes to `pending`, re-run |
| Headless exits 4 (Error) | Read stderr — likely provider/auth issue. Fix and re-run |
| Frontend tsc errors | Read error output, send cc with exact fix (type signature correction) |
| Frontend vite build fails | Similar — send cc with precise fix |
| `cargo test` fails | Read failure output, send cc with targeted fix |
| Daemon dies mid-headless | Re-export env vars, restart. workgraph.json state is persisted — resume from last checkpoint |
| SSE/flaky tests | Check broadcast setup, test with single subscriber within one test function |
| concurrent cc race | Never happens if we serialize — carefully wait for daemon turn to finish before next cc |
