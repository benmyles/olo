#!/usr/bin/env bash
# Deploy olo-desktop.com to Cloudflare.
#
#   tools/deploy-website.sh            stage website/dist, then deploy it
#   tools/deploy-website.sh --build    stage only
#   tools/deploy-website.sh --preview  deploy to a preview URL, not the site
#   tools/deploy-website.sh --yes      don't ask before deploying
#
# The site is the Cloudflare project "olo-desktop" (OLO_SITE_PROJECT names
# another), which serves website/dist as it is. The site needs no build:
# website/ is the page as it is served (website/README.md), and staging is
# copying what is served (index.html, index.md, llms.txt, assets/) into
# website/dist, without its tools and README. Deploying is Cloudflare's
# own tool's work (wrangler). Whatever is missing, this asks before it
# does anything about it: Node's npx to run
# wrangler (run from npm each time, nothing installed globally), and being
# signed in to Cloudflare (wrangler opens the browser). With
# CLOUDFLARE_API_TOKEN set (a token that may edit Workers or Pages), no
# sign-in is asked for: that is the way for a script with nobody at it.
#
# Which Cloudflare account: wrangler's own choice when there is one, else
# CLOUDFLARE_ACCOUNT_ID, else the id kept in website/.cloudflare-account
# (not in git; asked for once, and kept there).
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
site=$here/website
project=${OLO_SITE_PROJECT:-olo-desktop}
wrangler_version=4
build_only=0 preview=0 yes=0

for arg in "$@"; do
  case $arg in
    --build) build_only=1 ;;
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
fail() {
  echo "deploy-website: $*" >&2
  exit 1
}

# Yes or no, asked of the person at the terminal (Return is yes). With
# nobody there, the answer is no: nothing is installed unasked.
ask() {
  if [ "$yes" = 1 ] && [ "${2:-}" = deploy ]; then return 0; fi
  if [ ! -t 0 ]; then
    echo "$1 (nobody to ask: no)" >&2
    return 1
  fi
  local answer
  read -r -p "$1 [Y/n] " answer
  case ${answer:-y} in y | Y | yes | Yes) return 0 ;; *) return 1 ;; esac
}

# ---- what it needs ----------------------------------------------------------

if [ "$build_only" = 0 ] && ! command -v npx >/dev/null; then
  say "Node.js isn't installed (Cloudflare's wrangler runs on it)."
  if command -v mise >/dev/null; then
    ask "Install Node.js with mise (into your home folder)?" || fail "it needs npx (Node.js)"
    mise use --global node@lts
    eval "$(mise env -s bash)"
  elif command -v dnf >/dev/null; then
    ask "Install Node.js with dnf (it asks for your password)?" || fail "it needs npx (Node.js)"
    sudo dnf install -y nodejs npm
  else
    fail "it needs npx (Node.js): install it, then run this again"
  fi
  command -v npx >/dev/null || fail "Node.js went in, but npx isn't on PATH: open a new shell and run this again"
fi

# ---- build ------------------------------------------------------------------

say "Staging the site…"
rm -rf "$site/dist"
mkdir -p "$site/dist"
for f in index.html index.md llms.txt assets; do
  [ -e "$site/$f" ] || fail "website/ has no $f"
  cp -r "$site/$f" "$site/dist/"
done
files=$(find "$site/dist" -type f | wc -l)
size=$(du -sh "$site/dist" | cut -f1)
echo "website/dist: $files files, $size."
if [ "$build_only" = 1 ]; then exit 0; fi

# ---- Cloudflare -------------------------------------------------------------

# (From npm each time, at a pinned major version; npm keeps it cached.)
wrangler() { npx --yes "wrangler@$wrangler_version" "$@"; }

if [ ! -d "$HOME/.npm/_npx" ] || ! ls "$HOME"/.npm/_npx/*/node_modules/wrangler >/dev/null 2>&1; then
  say "Cloudflare's wrangler isn't here yet."
  ask "Fetch it from npm (kept in npm's cache, nothing installed globally)?" || fail "it needs wrangler to deploy"
fi

if [ -z "${CLOUDFLARE_ACCOUNT_ID:-}" ] && [ -s "$site/.cloudflare-account" ]; then
  CLOUDFLARE_ACCOUNT_ID=$(tr -d '[:space:]' <"$site/.cloudflare-account")
  export CLOUDFLARE_ACCOUNT_ID
fi

if [ -z "${CLOUDFLARE_API_TOKEN:-}" ] && ! wrangler whoami 2>/dev/null | grep -q "You are logged in"; then
  say "You aren't signed in to Cloudflare."
  ask "Sign in now (wrangler opens your browser)?" || fail "it needs to be signed in: run it again when you are, or set CLOUDFLARE_API_TOKEN"
  wrangler login
  wrangler whoami | grep -q "You are logged in" || fail "the sign-in didn't complete"
fi

# More than one account, and none said: asked once, and kept (not in git).
if [ -z "${CLOUDFLARE_ACCOUNT_ID:-}" ]; then
  accounts=$(wrangler whoami 2>/dev/null | grep -Eo '[0-9a-f]{32}' | sort -u)
  if [ "$(printf '%s\n' "$accounts" | grep -c .)" -gt 1 ]; then
    say "You have more than one Cloudflare account:"
    wrangler whoami 2>/dev/null | grep -E '[0-9a-f]{32}' || true
    [ -t 0 ] || fail "say which with CLOUDFLARE_ACCOUNT_ID"
    read -r -p "The account id that has $project: " CLOUDFLARE_ACCOUNT_ID
    [[ $CLOUDFLARE_ACCOUNT_ID =~ ^[0-9a-f]{32}$ ]] || fail "that isn't an account id"
    echo "$CLOUDFLARE_ACCOUNT_ID" >"$site/.cloudflare-account"
    export CLOUDFLARE_ACCOUNT_ID
  fi
fi

# What kind of project it is: a Pages project, or a Worker that serves
# static files. Asked of Cloudflare, so the script is right either way.
kind=worker
if wrangler pages project list 2>/dev/null | grep -qw "$project"; then kind=pages; fi

where="the live site"
[ "$preview" = 1 ] && where="a preview URL"
say "Ready to deploy website/dist to $where (Cloudflare $kind \"$project\")."
ask "Deploy?" deploy || fail "not deployed"

commit=$(git -C "$here" rev-parse --short HEAD 2>/dev/null || echo unknown)
dirty=$(git -C "$here" status --porcelain -- website 2>/dev/null | grep -c . || true)
message="olo $commit"
[ "$dirty" != 0 ] && message="$message (with uncommitted changes)"

if [ "$kind" = pages ]; then
  branch=main
  [ "$preview" = 1 ] && branch=preview
  wrangler pages deploy "$site/dist" --project-name "$project" --branch "$branch" --commit-hash "$commit" --commit-message "$message" --commit-dirty=true
else
  # (A Worker with nothing but static files: Cloudflare serves them; the
  # site is one page, with no 404 page of its own.)
  config=$(mktemp /var/tmp/olo-website-XXXXXX.jsonc)
  trap 'rm -f "$config"' EXIT
  cat >"$config" <<JSON
{
  "name": "$project",
  "compatibility_date": "2026-01-01",
  "assets": { "directory": "$site/dist", "not_found_handling": "none", "html_handling": "auto-trailing-slash" }
}
JSON
  if [ "$preview" = 1 ]; then
    wrangler versions upload --config "$config" --message "$message"
  else
    wrangler deploy --config "$config" --message "$message"
  fi
fi

say "Deployed."
[ "$preview" = 1 ] || echo "https://olo-desktop.com (a minute for every edge to have it)"
