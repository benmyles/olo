-- Olo's skill for agents on a Rex host: one command of the Olo desktop
-- (`olo open|op|screen|forward|send|sync|notify|clipboard …`, beside this file), asked
-- through Rex and answered here as if it had run here. It is `rex do`
-- underneath, and saves an agent the three things that took a loop or
-- a guess by hand: which client is the Olo desktop (the one connected,
-- and showing this session where several are: clients the server
-- still lists as lost are passed over), a command that takes longer
-- than Rex waits (waited for), and a picture the command made (a file
-- of the desktop: fetched whole and written here, its path here put
-- in the output where the desktop's stood).
local function fail(msg, code)
  io.stderr:write("olo: " .. msg .. "\n")
  os.exit(code or 1)
end

local cmd = os.getenv("OLO_BRIDGE_CMD") or ""
local args = {}
for i = 1, tonumber(os.getenv("OLO_BRIDGE_ARGC") or "0") do
  args[i] = os.getenv("OLO_BRIDGE_ARG_" .. i) or ""
end
local asked = ({
  open = function() return "olo.open", { target = args[1] or "" } end,
  op = function() return "olo.op", { args = args } end,
  screen = function() return "olo.screen", { args = args } end,
  forward = function() return "olo.forward", { args = args } end,
  -- (Paths as the command here meant them: whole, from where it was run.)
  send = function()
    local whole = {}
    for i, x in ipairs(args) do whole[i] = x:sub(1, 1) == "-" and x or ((x:sub(1, 1) == "/") and x or ((os.getenv("PWD") or ".") .. "/" .. x)) end
    return "olo.send", { args = whole }
  end,
  -- (Its folder made whole as `send`'s paths are; its words left as they are.)
  sync = function()
    local whole, words = {}, { ls = true, list = true, wait = true, stop = true, rm = true }
    for i, x in ipairs(args) do
      if x:sub(1, 1) == "-" or (i == 1 and words[x]) or x:sub(1, 1) == "/" then whole[i] = x
      else whole[i] = (os.getenv("PWD") or ".") .. "/" .. x end
    end
    return "olo.sync", { args = whole }
  end,
  notify = function() return "olo.notify", { title = args[1] or "", body = args[2] or "" } end,
  clipboard = function() return "olo.clipboard.set", { text = args[1] or "" } end,
  paste = function() return "olo.clipboard.get", {} end,
})[cmd]
if not asked then
  fail("usage: olo open URL | olo op ARGS… | olo screen ARGS… | olo forward ARGS… | olo send PATH… | olo sync ARGS… | olo notify TITLE [BODY] | olo clipboard TEXT | olo paste", 2)
end
local action, a = asked()

-- A path as the command here meant it: from where it was run.
local function here_path(p)
  if p:sub(1, 1) == "/" then return p end
  return (os.getenv("PWD") or ".") .. "/" .. p
end
local function base64_of(path)
  local b = rex.exec({ "sh", "-c", 'base64 < "$1" | tr -d "\n"', "sh", here_path(path) })
  if not (b and b.ok) then fail("couldn't read " .. path .. " to send it to the Olo desktop", 1) end
  return b.stdout or ""
end
-- op's standard input (kept by `olo`) and the files its flags read are
-- this host's: sent with it, as the desktop takes nothing of its own
-- for a host (`bridge::OpCarry`).
if cmd == "op" then
  local stdin = os.getenv("OLO_BRIDGE_STDIN")
  if stdin and stdin ~= "" then
    a.stdin = base64_of(stdin)
    os.remove(stdin)
  end
  local reads = { ["-i"] = true, ["--in-file"] = true, ["--template"] = true, ["--env-file"] = true }
  local files, i = {}, 1
  while i <= #args and args[i] ~= "--" do
    local name, value = args[i]:match("^(%-%-?[%w-]+)=(.*)$")
    name = name or args[i]
    if reads[name] then
      if not value then
        i = i + 1
        value = args[i]
      end
      if value then files[value] = base64_of(value) end
    end
    i = i + 1
  end
  if next(files) then a.files = files end
end

local session, block = os.getenv("REX_SESSION"), os.getenv("REX_BLOCK")
local list, err = rex.call("client.list", {})
-- (69: the desktop couldn't be asked, here or for this. The desktop
-- commands beside this go on to the host's own then.)
if not list then fail("the Rex server here didn't answer: " .. tostring(err), 69) end
local app, shown
for _, c in ipairs(list.clients or {}) do
  if c.connection_state == "connected" and c.info and c.info.kind == "app" and c.info.name == "Olo Rex" then
    app = app or c.client_id
    for _, s in ipairs(c.session_ids or {}) do
      if s == session then shown = c.client_id end
    end
  end
end
app = shown or app
if not app then fail("no Olo desktop is connected to this Rex server now (open this host in Olo's Rex)", 69) end

local function ask(name, with)
  local r, why = rex.call("client.perform", { client_id = app, action = name, args = with, session_id = session, block_id = block, timeout_ms = 30000 })
  if not r then fail(tostring(why), 69) end
  return r.result or {}
end

local r = ask(action, a)
while r.pending do r = ask("olo.wait", { id = r.pending }) end

-- A picture comes in pieces of base64 (a request through Rex has a
-- size): each is written beside where the picture goes and decoded by
-- the host's own `base64`. (Decoded here, letter by letter, a piece of
-- 600 kB overflowed this Lua's stack.)
local out = r.stdout or ""
local dir = os.getenv("OLO_BRIDGE_DIR") or "/var/tmp/olo-rex-bridge"
-- What the command printed that isn't text (a document's bytes), after
-- its text.
local printed = {}
-- (`op`'s: whether it may replace a file with something in it, and the
-- mode it gives one, as `op` itself would here.)
local force, mode = false, "600"
if cmd == "op" then
  for i, x in ipairs(args) do
    if x == "--" then break end
    if x == "--force" or x == "-f" or x == "--force=true" then force = true end
    mode = x:match("^%-%-file%-mode=(%d+)$") or (x == "--file-mode" and args[i + 1]) or mode
  end
end
for _, f in ipairs(r.files or {}) do
  local name = (f.path:match("([^/]+)$") or "picture.png"):gsub("[^%w%._-]", "_")
  -- (A file the command here asked for, `op read -o FILE`: where it
  -- said, readable by its owner alone; what it printed: in its place.)
  local here = f.to and here_path(f.to) or dir .. "/" .. name
  if f.stdout then here = dir .. "/stdout." .. f.id end
  local part = dir .. "/" .. f.id .. ".b64"
  local ready = true
  if f.to then
    local existing = io.open(here, "rb")
    local size = existing and existing:seek("end") or 0
    if existing then existing:close() end
    if size > 0 and not force then
      io.stderr:write("olo: " .. f.to .. " already exists and isn't empty: not replaced (--force replaces it)\n")
      ready = false
    else
      local made = rex.exec({ "sh", "-c", 'umask 077 && : > "$1"', "sh", here })
      ready = made and made.ok
    end
  else
    local written = io.open(here, "wb")
    if written then written:close() else ready = false end
  end
  if ready then
    local offset, ok = 0, true
    repeat
      local piece = ask("olo.file", { id = f.id, offset = offset })
      local b64 = io.open(part, "wb")
      b64:write(piece.base64 or "")
      b64:close()
      local decoded = rex.exec({ "sh", "-c", 'base64 -d "$1" >> "$2"', "sh", part, here })
      ok = decoded and decoded.ok
      offset = offset + (piece.count or 0)
    until not ok or piece.done or (piece.count or 0) == 0
    os.remove(part)
    if ok and f.to then
      rex.exec({ "chmod", mode, here })
    elseif ok and f.stdout then
      printed[#printed + 1] = here
    elseif ok then
      local at, to = out:find(f.path, 1, true)
      while at do
        out = out:sub(1, at - 1) .. here .. out:sub(to + 1)
        at, to = out:find(f.path, at + #here, true)
      end
    else
      io.stderr:write("olo: couldn't decode " .. here .. " (is `base64` on this host?)\n")
    end
  elseif not f.to then
    io.stderr:write("olo: couldn't write " .. here .. "\n")
  end
end
io.write(out)
for _, p in ipairs(printed) do
  local bytes = io.open(p, "rb")
  if bytes then
    io.write(bytes:read("a"))
    bytes:close()
  end
  os.remove(p)
end
io.stdout:flush()
io.stderr:write(r.stderr or "")
local code = type(r.data) == "table" and tonumber(r.data.code) or nil
os.exit(r.ok and 0 or ((code and code ~= 0) and code or 1))
