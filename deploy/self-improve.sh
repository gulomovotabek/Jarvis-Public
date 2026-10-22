#!/usr/bin/env bash
set -euo pipefail

MODE=${1:-}
JOB_ID=${2:-}
REF=${3:-}
case "$MODE" in
  improve | deploy) ;;
  *)
    echo "usage: self-improve.sh improve <job-id> | deploy <job-id> <branch>" >&2
    exit 2
    ;;
esac
if [[ ! "$JOB_ID" =~ ^[A-Za-z0-9][A-Za-z0-9-]{0,63}$ ]]; then
  echo "bad job id: $JOB_ID" >&2
  exit 2
fi
if [[ $MODE == deploy ]] && { [[ ! "$REF" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$ ]] || [[ "$REF" == *..* || "$REF" == *//* || "$REF" == */ || "$REF" == *. ]]; }; then
  echo "bad branch: $REF" >&2
  exit 2
fi

PROD=${JARVIS_PROD:-/opt/jarvis}
DATA=${JARVIS_DATA_DIR:-$PROD/data}
REPO=${SELF_IMPROVE_REPO:-$HOME/jarvis}
WORK=${SELF_IMPROVE_WORKTREE:-$HOME/jarvis-self}
TARGET=${SELF_IMPROVE_TARGET_DIR:-$HOME/.cache/jarvis-self-target}
CLAUDE_BIN=${CLAUDE_CODE_BIN:-claude}
MODEL=${SELF_IMPROVE_MODEL:-${CLAUDE_CODE_MODEL:-claude-opus-5-5}}
MAX_TURNS=${SELF_IMPROVE_MAX_TURNS:-80}
MAX_BUDGET=${SELF_IMPROVE_MAX_BUDGET_USD:-5}
AGENT_TIMEOUT=${SELF_IMPROVE_AGENT_TIMEOUT_SECS:-3600}
JOB=$DATA/self_improve/$JOB_ID
BRANCH=jarvis/$JOB_ID
SYSTEM_PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH=$SYSTEM_PATH:$HOME/.cargo/bin
export GIT_TERMINAL_PROMPT=0
export GIT_SSH_COMMAND="ssh -o BatchMode=yes"
GIT=(git -c core.hooksPath=/dev/null -c core.fsmonitor=false)

mkdir -p "$JOB" "$DATA/notices"
exec >>"$JOB/log" 2>&1

finished=0
current=starting

state() {
  current=$1
  printf '%s' "$1" >"$JOB/.state.tmp"
  mv -f "$JOB/.state.tmp" "$JOB/state"
  echo "[$(date -u +%FT%TZ)] $1"
}

notice() {
  local name
  name="$(date +%s%N)-$$"
  printf '%s' "$1" >"$DATA/notices/.$name.tmp"
  mv -f "$DATA/notices/.$name.tmp" "$DATA/notices/$name.txt"
}

fail() {
  finished=1
  state "failed: $1"
  notice "Self-improve $JOB_ID failed: $1"$'\n'"Log: $JOB/log"
  exit 1
}

on_exit() {
  local rc=$? at=$current
  if [[ $finished -eq 0 ]]; then
    finished=1
    state "aborted (exit $rc) during: $at"
    notice "Self-improve $JOB_ID aborted (exit $rc) during: $at"$'\n'"Log: $JOB/log"
  fi
}
trap on_exit EXIT
trap 'exit 143' TERM INT

sandboxed() {
  bwrap --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp \
    --tmpfs "$HOME" --tmpfs "$PROD" \
    --ro-bind "$HOME/.rustup" "$HOME/.rustup" \
    --bind "$HOME/.cargo" "$HOME/.cargo" \
    --ro-bind "$HOME/.cargo/bin" "$HOME/.cargo/bin" \
    --ro-bind "$REPO" "$REPO" \
    --bind "$WORK" "$WORK" \
    --bind "$TARGET" "$TARGET" \
    --unshare-all --share-net --die-with-parent --chdir "$WORK" \
    -- env -i HOME="$HOME" PATH="$SYSTEM_PATH:$HOME/.cargo/bin" LANG=C.UTF-8 \
    CARGO_TARGET_DIR="$TARGET" JARVIS_GIT_SHA="${sha:-}" \
    nice -n 10 "$@"
}

state "preparing"
mkdir -p "$DATA/self_improve"
exec 9>"$DATA/self_improve/.lock"
flock -n 9 || fail "another self-improve job is running"
[[ ! -e "$DATA/deploy_pending" ]] || fail "a previous deploy is still in probation ($DATA/deploy_pending exists)"

command -v bwrap >/dev/null || fail "bubblewrap (bwrap) is not installed"
command -v socat >/dev/null || fail "socat is not installed"
bwrap --ro-bind / / --dev /dev --proc /proc --unshare-all true || fail "bubblewrap cannot create a sandbox (AppArmor userns profile missing?)"
[[ -d "$REPO/.git" ]] || fail "no git clone at $REPO"
[[ -d "$HOME/.rustup" && -d "$HOME/.cargo/bin" ]] || fail "rust toolchain not found under $HOME"
mkdir -p "$TARGET"

state "fetching"
"${GIT[@]}" -C "$REPO" fetch --prune origin || fail "git fetch failed"
if [[ $MODE == improve ]]; then
  base=$(tr -d '[:space:]' 2>/dev/null <"$PROD/bin/VERSION" || true)
  if [[ -z "$base" ]] || ! "${GIT[@]}" -C "$REPO" cat-file -e "$base^{commit}" 2>/dev/null; then
    base=$("${GIT[@]}" -C "$REPO" rev-parse "refs/remotes/origin/main^{commit}")
  fi
else
  base=$("${GIT[@]}" -C "$REPO" rev-parse --verify -q "refs/remotes/origin/$REF^{commit}") || fail "branch origin/$REF not found"
fi

"${GIT[@]}" -C "$REPO" worktree prune
if [[ ! -e "$WORK" ]]; then
  "${GIT[@]}" -C "$REPO" worktree add -q --detach "$WORK" "$base" || fail "git worktree add failed"
elif [[ ! -f "$WORK/.git" ]]; then
  fail "$WORK exists but is not a git worktree of $REPO"
fi
if [[ $MODE == improve ]]; then
  "${GIT[@]}" -C "$WORK" checkout -q --force -B "$BRANCH" "$base"
else
  "${GIT[@]}" -C "$WORK" checkout -q --force --detach "$base"
fi
"${GIT[@]}" -C "$WORK" clean -fdxq
gitfile=$(cat "$WORK/.git")

if [[ $MODE == improve ]]; then
  state "agent running (base $(cut -c1-12 <<<"$base"))"
  python3 - "$HOME" "$PROD" "$WORK" "$TARGET" "$JOB" <<'PY'
import json, sys
home, prod, work, target, job = sys.argv[1:6]
secret = [prod, f"{home}/.ssh", f"{home}/.claude", f"{home}/.claude.json", f"{home}/.config",
          f"{home}/.git-credentials", f"{home}/.bash_history", f"{home}/.cargo/credentials.toml"]
protected = [f"{work}/.git", f"{work}/.claude", f"{work}/.mcp.json", f"{work}/.github", f"{work}/deploy",
             f"{work}/build.rs", f"{work}/src/bin/guard.rs", f"{home}/.cargo/bin", f"{home}/.cargo/config.toml",
             f"{home}/.cargo/config"]
deny = []
for path in secret:
    deny += [f"Read(/{path})", f"Read(/{path}/**)", f"Edit(/{path})", f"Edit(/{path}/**)"]
deny += ["Read(//**/.env)", "Edit(//**/.env)"]
for path in protected:
    deny += [f"Edit(/{path})", f"Edit(/{path}/**)", f"Write(/{path})", f"Write(/{path}/**)"]
settings = {
    "permissions": {
        "allow": ["Read", "Glob", "Grep", "Bash", "Edit(./**)", "Write(./**)"],
        "deny": deny,
    },
    "disableAllHooks": True,
    "sandbox": {
        "enabled": True,
        "failIfUnavailable": True,
        "autoAllowBashIfSandboxed": True,
        "allowUnsandboxedCommands": False,
        "filesystem": {
            "denyRead": secret,
            "allowWrite": [target, f"{home}/.cargo"],
            "denyWrite": protected,
        },
        "network": {"allowedDomains": ["crates.io", "index.crates.io", "static.crates.io"]},
    },
}
json.dump(settings, open(f"{job}/settings.json", "w"), indent=2)
json.dump({"mcpServers": {}}, open(f"{job}/mcp.json", "w"))
task = open(f"{job}/task.md").read().strip()
prompt = f"""You are changing the source code of Jarvis, the Rust Telegram bot in the current directory.
Read CLAUDE.md first for the architecture, then implement the task at the end.

Rules:
- Work only inside this repository. Do not commit, push, create branches or touch git config; the harness does that.
- Keep the change minimal and in the existing style. Do not add explanatory comments or docstrings.
- Do not modify src/bin/guard.rs, build.rs, deploy/, .github/, .claude/, .mcp.json, .cargo/, rust-toolchain files, or code that loads secrets/.env.
- Database changes must be additive (new tables, or new columns with defaults). Never drop or rename existing tables or columns.
- The self_improve, self_deploy, self_improve_status and restart_self MCP tools and the --version flag must keep working; the harness refuses builds without them.
- Run `cargo build --release` and `cargo test --release` before finishing. Both must pass.
- If the task is unclear, unsafe, or would weaken security or privacy, change nothing and explain why.
- Finish with a short plain-text summary of what changed and how you verified it. It becomes the commit message body.

Task:
{task}
"""
open(f"{job}/prompt.md", "w").write(prompt)
PY

  set +e
  (
    cd "$WORK"
    timeout "$AGENT_TIMEOUT" env -i HOME="$HOME" PATH="$SYSTEM_PATH:$HOME/.cargo/bin" USER="${USER:-}" \
      SHELL=/bin/bash LANG=C.UTF-8 TERM=dumb CARGO_TARGET_DIR="$TARGET" \
      "$CLAUDE_BIN" -p --model "$MODEL" --max-turns "$MAX_TURNS" --max-budget-usd "$MAX_BUDGET" \
      --permission-mode dontAsk --setting-sources project --settings "$JOB/settings.json" \
      --strict-mcp-config --mcp-config "$JOB/mcp.json" --tools "Read,Glob,Grep,Bash,Edit,Write" \
      --no-session-persistence --output-format json <"$JOB/prompt.md" >"$JOB/agent.json"
  )
  agent_rc=$?
  set -e
  agent_info=$(python3 - "$JOB/agent.json" "$JOB/summary.md" <<'PY'
import json, sys
try:
    data = json.load(open(sys.argv[1]))
except Exception as e:
    print(f"unparseable agent output ({e})")
    sys.exit(1)
open(sys.argv[2], "w").write(str(data.get("result") or "").strip() + "\n")
print(f"{data.get('subtype')}, {data.get('num_turns')} turns, ${float(data.get('total_cost_usd') or 0):.2f}")
sys.exit(0 if data.get("subtype") == "success" and not data.get("is_error") else 1)
PY
  ) && agent_ok=1 || agent_ok=0
  echo "agent exit=$agent_rc: $agent_info"
  [[ $agent_ok -eq 1 && $agent_rc -eq 0 ]] || fail "agent did not finish (exit $agent_rc, $agent_info)"

  [[ "$(cat "$WORK/.git" 2>/dev/null)" == "$gitfile" ]] || fail "agent modified $WORK/.git"
  "${GIT[@]}" -C "$WORK" add -A
  if "${GIT[@]}" -C "$WORK" diff --cached --quiet; then
    finished=1
    state "noop: agent made no changes"
    notice "Self-improve $JOB_ID: agent made no changes ($agent_info)."$'\n\n'"$(head -c 1500 "$JOB/summary.md")"
    exit 0
  fi
  protected=$("${GIT[@]}" -C "$WORK" diff --cached --name-only | grep -E '^(src/bin/guard\.rs|build\.rs|deploy/|\.github/|\.claude/|\.mcp\.json|\.cargo/|rust-toolchain|\.gitmodules)' || true)
  [[ -z "$protected" ]] || fail "agent touched protected paths: $(tr '\n' ' ' <<<"$protected")"
  python3 - "$JOB" <<'PY'
import sys
job = sys.argv[1]
title = (open(f"{job}/task.md").read().strip().splitlines() or ["self-improve"])[0].strip()
if len(title) > 64:
    title = title[:63].rstrip() + "…"
summary = open(f"{job}/summary.md").read().strip()
open(f"{job}/commit_msg", "w").write(f"jarvis: {title}\n\n{summary}\n\nSelf-improve job: {job.rsplit('/', 1)[-1]}\n")
PY
  "${GIT[@]}" -C "$WORK" -c user.name=Jarvis -c user.email=jarvis@users.noreply.github.com \
    commit -q --no-gpg-sign -F "$JOB/commit_msg"
fi

sha=$("${GIT[@]}" -C "$WORK" rev-parse --short=12 HEAD)
label=$([[ $MODE == improve ]] && echo "$BRANCH" || echo "origin/$REF")

state "building $sha"
sandboxed cargo build --release --locked || fail "release build failed for $sha"
state "testing $sha"
sandboxed cargo test --release --locked || fail "tests failed for $sha"
state "smoke testing $sha"
built_version=$(sandboxed "$TARGET/release/telegram-codex-jarvis" --version | tr -d '[:space:]') || true
[[ "$built_version" == "$sha" ]] || fail "new binary --version printed '$built_version', expected $sha"
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' |
  sandboxed timeout 20 env DATABASE_URL="sqlite:///tmp/smoke.db?mode=rwc" "$TARGET/release/mcp-telegram" >"$JOB/smoke.json" ||
  fail "mcp-telegram smoke test exited non-zero"
python3 - "$JOB/smoke.json" <<'PY' || fail "mcp-telegram smoke test: tools/list incomplete"
import json, sys
replies = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
tools = {t["name"] for r in replies if r.get("id") == 2 for t in r["result"]["tools"]}
required = {"self_improve", "self_deploy", "self_improve_status", "restart_self", "bot_send_message"}
print(f"smoke: {len(tools)} tools")
sys.exit(0 if required <= tools else 1)
PY

if [[ $MODE == improve ]]; then
  state "pushing $BRANCH"
  "${GIT[@]}" -C "$REPO" push -q origin "refs/heads/$BRANCH:refs/heads/$BRANCH" || fail "git push of $BRANCH failed"
fi

state "installing $sha"
files=("$PROD/bin/telegram-codex-jarvis" "$PROD/bin/mcp-telegram" "$PROD/bin/VERSION")
install -m 0755 "$TARGET/release/telegram-codex-jarvis" "$PROD/bin/telegram-codex-jarvis.new"
install -m 0755 "$TARGET/release/mcp-telegram" "$PROD/bin/mcp-telegram.new"
printf '%s\n' "$sha" >"$PROD/bin/VERSION.new"
for prompt in "$WORK"/prompts/*; do
  [[ -f "$prompt" ]] || continue
  dest="$PROD/prompts/$(basename "$prompt")"
  cp "$prompt" "$dest.new"
  files+=("$dest")
done
for f in "${files[@]}"; do
  if [[ -e "$f" ]]; then cp -p "$f" "$f.prev"; else rm -f "$f.prev"; fi
done
python3 - "$DATA/.deploy_pending.tmp" "$sha" "$label" "$JOB_ID" "${files[@]}" <<'PY'
import json, sys
out, sha, branch, job, *files = sys.argv[1:]
json.dump({"sha": sha, "branch": branch, "job": job, "files": files}, open(out, "w"))
PY
mv -f "$DATA/.deploy_pending.tmp" "$DATA/deploy_pending"
for f in "${files[@]}"; do
  mv -f "$f.new" "$f"
done

finished=1
state "restarting into $sha"
summary=""
[[ $MODE == improve ]] && summary=$'\n\n'"$(head -c 1500 "$JOB/summary.md")"
notice "Self-improve $JOB_ID: $label built, tested and installed as $sha. Restarting now; it is rolled back automatically if the new build fails health probation.$summary"
printf 'deploy %s (%s)' "$sha" "$label" >"$DATA/.restart_requested.tmp"
mv -f "$DATA/.restart_requested.tmp" "$DATA/restart_requested"
