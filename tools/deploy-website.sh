#!/usr/bin/env bash
# Deploy olo-desktop.com (website/) to Cloudflare Pages, setting up what
# isn't there yet.
#
#   tools/deploy-website.sh            check, set up, stage website/dist, deploy
#   tools/deploy-website.sh --check    check, and say what a deploy would do;
#                                      change nothing
#   tools/deploy-website.sh --build    stage website/dist only (no Cloudflare)
#   tools/deploy-website.sh --preview  deploy to a preview URL, not the site
#   tools/deploy-website.sh --yes      don't ask before changing anything
#
# At its start, before anything is changed, it checks that:
#
#   - the commands it needs are here: Cloudflare's `cf` and `wrangler` (or
#     Node's `npx` to run wrangler), `python3` and `git`;
#   - `cf` is signed in (`cf auth login`), and to the right account: the one
#     whose id is CLOUDFLARE_ACCOUNT_ID, else the id kept in
#     website/.cloudflare-account (not in git). The first time, with neither,
#     it names the account cf is signed in to and asks before keeping it
#     there; signed in to another account later, it stops;
#   - `wrangler` is signed in (`wrangler login`) to that same account.
#
# Then each run makes sure of each of these, in turn, and changes only what
# isn't so (running it again changes nothing but the deployment):
#
#   1. The Pages project (OLO_SITE_PROJECT, default "olo-desktop") is
#      there: made with `cf pages create` if not, a Direct Upload project
#      whose production branch is "main".
#   2. website/dist holds what is served (index.html, index.md, llms.txt,
#      assets/), copied from website/: the site needs no build.
#   3. It is deployed: `wrangler pages deploy` (cf has no command that
#      uploads a deployment's files), to the production branch, or with
#      --preview to the branch "preview" (a URL of its own).
#   4. Each custom domain (OLO_SITE_DOMAINS, default "olo-desktop.com
#      www.olo-desktop.com"; empty for none) is attached to the project,
#      and, where its zone is in the account, has a proxied CNAME record
#      to the project's pages.dev name. A record of that name that is
#      anything else is left as it is, and said.
#
# It asks before each change (Return is yes); with nobody at the terminal
# the answer is no, unless --yes.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
site=$here/website
project=${OLO_SITE_PROJECT:-olo-desktop}
domains=${OLO_SITE_DOMAINS-olo-desktop.com www.olo-desktop.com}
mode=deploy preview=0 yes=0

for arg in "$@"; do
  case $arg in
    --check) mode=check ;;
    --build) mode=build ;;
    --preview) preview=1 ;;
    --yes | -y) yes=1 ;;
    -h | --help)
      sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "deploy-website: no option $arg (--help lists them)" >&2
      exit 2
      ;;
  esac
done

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
ok() { printf '  \033[32m✓\033[0m %s\n' "$*"; }
todo() { printf '  \033[33m•\033[0m %s\n' "$*"; }
fail() {
  echo "deploy-website: $*" >&2
  exit 1
}

# Yes or no, asked of the person at the terminal (Return is yes). With
# nobody there, the answer is no, unless --yes.
ask() {
  [ "$yes" = 1 ] && return 0
  if [ ! -t 0 ]; then
    echo "  $1 (nobody to ask: no; --yes answers yes)" >&2
    return 1
  fi
  local answer
  read -r -p "  $1 [Y/n] " answer
  case ${answer:-y} in y | Y | yes | Yes) return 0 ;; *) return 1 ;; esac
}

# `json 'PYTHON'`: the JSON on stdin as `d`.
json() { python3 -c "import json, sys; d = json.load(sys.stdin); $1"; }

# A command that asks Cloudflare, given two minutes at most (so a network
# that doesn't answer is said, not waited on).
limited() {
  if command -v timeout >/dev/null; then timeout 120 "$@"; else "$@"; fi
}

# What is served, copied into website/dist.
stage() {
  rm -rf "$site/dist"
  mkdir -p "$site/dist"
  local f
  for f in index.html index.md llms.txt assets; do
    [ -e "$site/$f" ] || fail "website/ has no $f"
    cp -r "$site/$f" "$site/dist/"
  done
  files=$(find "$site/dist" -type f | wc -l)
  size=$(du -sh "$site/dist" | cut -f1)
}

if [ "$mode" = build ]; then
  stage
  echo "website/dist: $files files, $size."
  exit 0
fi

# ---- At its start: the commands, and both signed in to the right account ----

say "Checking the commands and the account"
# (pnpm's global commands, where cf and wrangler are put, may not be on PATH.)
PATH="$PATH:${PNPM_HOME:-$HOME/.local/share/pnpm}/bin"
missing=()
command -v python3 >/dev/null || missing+=("python3")
command -v git >/dev/null || missing+=("git")
command -v cf >/dev/null || missing+=("cf (Cloudflare's CLI; then: cf auth login)")
# (The array is `wr`, and the function below `run_wrangler`: a function named
# as the command would call itself.)
if command -v wrangler >/dev/null; then
  wr=(wrangler)
elif command -v npx >/dev/null; then
  wr=(npx --yes wrangler@4)
else
  wr=()
  missing+=("wrangler (or Node's npx to run it; then: wrangler login)")
fi
if [ ${#missing[@]} -gt 0 ]; then
  printf 'deploy-website: it needs, and did not find:\n' >&2
  printf '  %s\n' "${missing[@]}" >&2
  exit 1
fi
ok "python3, git, cf and ${wr[*]} are here"
# (Run from website/, so no other project's wrangler config is read.)
run_wrangler() { (cd "$site" && limited "${wr[@]}" "$@"); }

# cf: signed in, and its accounts ("ID NAME" a line).
who=$(limited cf auth whoami 2>/dev/null) || fail "cf isn't signed in (or Cloudflare didn't answer): cf auth login"
echo "$who" | json 'sys.exit(0 if d.get("authenticated") and d.get("tokenValid", True) else 1)' ||
  fail "cf isn't signed in: cf auth login"
accounts=$(echo "$who" | json 'print("\n".join(a["id"] + " " + a.get("name", "") for a in d.get("accounts", [])))')
[ -n "$accounts" ] || fail "cf is signed in, but to no account"

# The right account: said, or kept from the first time.
want=${CLOUDFLARE_ACCOUNT_ID:-}
kept=$site/.cloudflare-account
if [ -z "$want" ] && [ -s "$kept" ]; then want=$(tr -d '[:space:]' <"$kept"); fi
if [ -z "$want" ]; then
  if [ "$(printf '%s\n' "$accounts" | grep -c .)" != 1 ]; then
    printf 'deploy-website: cf is signed in to several accounts:\n' >&2
    printf '%s\n' "$accounts" | sed 's/^/  /' >&2
    fail "say which with CLOUDFLARE_ACCOUNT_ID, or put its id in website/.cloudflare-account"
  fi
  want=${accounts%% *}
  if [ "$mode" = check ]; then
    todo "no account is kept yet: a deploy would ask to keep \"${accounts#* }\" ($want) in website/.cloudflare-account"
  else
    ask "Deploy to the Cloudflare account \"${accounts#* }\" ($want), and keep it as the site's (website/.cloudflare-account)?" ||
      fail "no account chosen: nothing done"
    echo "$want" >"$kept"
  fi
fi
[[ $want =~ ^[0-9a-f]{32}$ ]] || fail "\"$want\" isn't a Cloudflare account id (CLOUDFLARE_ACCOUNT_ID, or website/.cloudflare-account)"
name=$(printf '%s\n' "$accounts" | sed -n "s/^$want //p")
if ! printf '%s\n' "$accounts" | grep -q "^$want "; then
  printf 'deploy-website: cf is signed in to another account than the site'"'"'s (%s):\n' "$want" >&2
  printf '%s\n' "$accounts" | sed 's/^/  /' >&2
  fail "sign in to the site's account (cf auth login), or say another with CLOUDFLARE_ACCOUNT_ID"
fi
export CLOUDFLARE_ACCOUNT_ID=$want
ok "cf is signed in to the site's account: \"$name\" ($want)"

# wrangler: signed in, and to the same account.
wwho=$(run_wrangler whoami 2>&1) || true
echo "$wwho" | grep -q "You are logged in" || fail "wrangler isn't signed in (or Cloudflare didn't answer): wrangler login"
echo "$wwho" | grep -q "$want" || fail "wrangler is signed in, but not to the site's account ($want): wrangler login"
ok "wrangler is signed in to the same account"

# ---- 1. the Pages project ----------------------------------------------------

say "The Pages project \"$project\""
err=$(mktemp /var/tmp/deploy-website-XXXXXX)
trap 'rm -f "$err"' EXIT
project_json() { limited cf pages get "$project" 2>"$err"; }
if proj=$(project_json); then
  ok "it is there"
else
  grep -qE '8000007|404' "$err" || fail "cf pages get: $(cat "$err")"
  todo "it isn't there: to make, production branch \"main\""
  if [ "$mode" = check ]; then
    proj=""
  else
    ask "Make the Pages project \"$project\"?" || fail "not made: nothing deployed"
    limited cf pages create --name "$project" --production-branch main >/dev/null
    proj=$(project_json) || fail "cf pages create made no project: $(cat "$err")"
    ok "made"
  fi
fi
if [ -n "$proj" ]; then
  subdomain=$(echo "$proj" | json 'print(d["subdomain"])')
  production=$(echo "$proj" | json 'print(d.get("production_branch") or "main")')
  ok "https://$subdomain (production branch \"$production\")"
else
  subdomain="$project.pages.dev" production=main
fi

# ---- 2 and 3. staged and deployed ---------------------------------------------

say "The deployment"
stage
ok "website/dist: $files files, $size"
branch=$production
[ "$preview" = 1 ] && branch=preview
commit=$(git -C "$here" rev-parse --short HEAD 2>/dev/null || echo unknown)
dirty=false
[ -n "$(git -C "$here" status --porcelain -- website 2>/dev/null)" ] && dirty=true
message="olo $commit"
[ "$dirty" = true ] && message="$message (website/ with uncommitted changes)"
where="the site (branch \"$branch\")"
[ "$preview" = 1 ] && where="a preview URL (branch \"preview\")"
if [ "$mode" = check ]; then
  todo "a deploy would send website/dist to $where, as \"$message\""
else
  ask "Deploy website/dist to $where?" || fail "not deployed"
  # (The upload is given ten minutes, not `limited`'s two.)
  out=$(cd "$site" && "${wr[@]}" pages deploy "$site/dist" --project-name "$project" --branch "$branch" \
    --commit-hash "$commit" --commit-message "$message" --commit-dirty="$dirty" 2>&1) || {
    echo "$out" >&2
    fail "wrangler pages deploy failed"
  }
  url=$(echo "$out" | grep -Eo 'https://[a-z0-9.-]+\.pages\.dev' | tail -1 || true)
  ok "deployed${url:+: $url}"
fi

# ---- 4. its domains (the production site's only) --------------------------------

if [ "$preview" = 1 ] || [ -z "$domains" ]; then
  say "Done."
  exit 0
fi

say "Its domains"
# What cf lists, as lines: the command, then the Python that prints them.
# A list that can't be had stops the run with what cf said (a guess at
# "nothing there" would make what is there a second time).
listed() {
  local how=$1 out
  shift
  out=$(limited cf "$@" 2>"$err") || fail "cf $*: $(cat "$err")"
  echo "$out" | json "$how"
}
# (`list` takes the project as --project-name; `create` as its argument.)
domain_list() { listed 'print("\n".join(x["name"] + " " + x.get("status", "?") for x in d))' pages domains list --project-name "$project"; }
attached=""
if [ -n "$proj" ]; then attached=$(domain_list); fi
zones=$(listed 'print("\n".join(z["name"] for z in d))' zones list)
for d in $domains; do
  # Attached to the project?
  if printf '%s\n' "$attached" | grep -q "^$d "; then
    ok "$d is the project's ($(printf '%s\n' "$attached" | grep "^$d " | cut -d' ' -f2))"
  elif [ "$mode" = check ]; then
    todo "$d: to attach to the project"
  elif ask "Attach $d to the project?"; then
    limited cf pages domains create "$project" --name "$d" >/dev/null
    ok "$d attached"
  else
    todo "$d not attached"
    continue
  fi
  # Its DNS: the zone it is in, if that zone is this account's.
  zone=""
  for z in $zones; do
    if [ "$d" = "$z" ] || [ "${d%."$z"}" != "$d" ]; then
      [ ${#z} -gt ${#zone} ] && zone=$z
    fi
  done
  if [ -z "$zone" ]; then
    todo "$d: its zone isn't in this account; point a CNAME at $subdomain where its DNS is"
    continue
  fi
  records=$(listed 'print("\n".join(r["type"] + " " + r.get("content", "") for r in d))' dns records list --zone "$zone" --name "$d")
  if printf '%s\n' "$records" | grep -qix "CNAME $subdomain"; then
    ok "$d: its CNAME to $subdomain is there"
  elif [ -n "$records" ]; then
    todo "$d already has records ($(printf '%s' "$records" | tr '\n' ';')): left as they are; Pages needs a CNAME to $subdomain"
  elif [ "$mode" = check ]; then
    todo "$d: a proxied CNAME to $subdomain, to make (zone $zone)"
  elif ask "Make $d a proxied CNAME to $subdomain (zone $zone)?"; then
    body=$(python3 -c 'import json, sys; print(json.dumps({"type": "CNAME", "name": sys.argv[1], "content": sys.argv[2], "proxied": True, "ttl": 1, "comment": "olo-desktop.com on Cloudflare Pages (tools/deploy-website.sh)"}))' "$d" "$subdomain")
    limited cf dns records create --zone "$zone" --body "$body" >/dev/null
    ok "$d: CNAME to $subdomain made"
  else
    todo "$d: no CNAME made"
  fi
done

# A domain that isn't active yet, its record there: Pages is asked to look
# again now (it checked when the domain was attached, before its record was
# made, and its next look of its own is some minutes off), and how each
# stands is said: its record seen ("verification"), then its certificate.
if [ "$mode" != check ]; then
  for d in $(domain_list | grep -v ' active$' | cut -d' ' -f1); do
    limited cf pages domains validation retry "$d" --project-name "$project" >/dev/null 2>&1 || true
  done
  waiting=0
  while read -r d state seen certificate; do
    [ -n "$d" ] || continue
    if [ "$state" = active ]; then
      ok "https://$d is live"
    else
      todo "$d: $state (its record seen: $seen; its certificate: $certificate)"
      waiting=1
    fi
  done < <(listed 'print("\n".join(" ".join([x["name"], x.get("status", "?"), (x.get("verification_data") or {}).get("status", "?"), (x.get("validation_data") or {}).get("status", "?")]) for x in d))' pages domains list --project-name "$project")
  if [ "$waiting" = 1 ]; then
    echo "  (Cloudflare issues a new domain's certificate by itself, in some minutes; until then the"
    echo "   domain answers with its error 522. https://$subdomain is live meanwhile; --check says how it stands.)"
  fi
fi
if [ "$mode" = check ]; then
  say "Checked: nothing was changed. (• is what a deploy would do.)"
  exit 0
fi
say "Done."
echo "https://$subdomain$(for d in $domains; do printf ', https://%s' "$d"; done)"
