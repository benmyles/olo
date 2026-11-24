-- Olo's skill for agents on a Rex host: one command of the Olo desktop
-- (`olo open|op|screen|notify|clipboard …`, beside this file), asked
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
  notify = function() return "olo.notify", { title = args[1] or "", body = args[2] or "" } end,
  clipboard = function() return "olo.clipboard.set", { text = args[1] or "" } end,
  paste = function() return "olo.clipboard.get", {} end,
})[cmd]
if not asked then
  fail("usage: olo open URL | olo op ARGS… | olo screen ARGS… | olo notify TITLE [BODY] | olo clipboard TEXT | olo paste", 2)
end
local action, a = asked()

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
for _, f in ipairs(r.files or {}) do
  local name = (f.path:match("([^/]+)$") or "picture.png"):gsub("[^%w%._-]", "_")
  local here = dir .. "/" .. name
  local written = io.open(here, "wb")
  if written then
    written:close()
    local offset, ok = 0, true
    repeat
      local piece = ask("olo.file", { id = f.id, offset = offset })
      local part = io.open(here .. ".b64", "wb")
      part:write(piece.base64 or "")
      part:close()
      local decoded = rex.exec({ "sh", "-c", 'base64 -d "$1" >> "$2"', "sh", here .. ".b64", here })
      ok = decoded and decoded.ok
      offset = offset + (piece.count or 0)
    until not ok or piece.done or (piece.count or 0) == 0
    os.remove(here .. ".b64")
    if ok then
      local at, to = out:find(f.path, 1, true)
      while at do
        out = out:sub(1, at - 1) .. here .. out:sub(to + 1)
        at, to = out:find(f.path, at + #here, true)
      end
    else
      io.stderr:write("olo: couldn't decode " .. here .. " (is `base64` on this host?)\n")
    end
  else
    io.stderr:write("olo: couldn't write " .. here .. "\n")
  end
end
io.write(out)
io.stdout:flush()
io.stderr:write(r.stderr or "")
local code = type(r.data) == "table" and tonumber(r.data.code) or nil
os.exit(r.ok and 0 or ((code and code ~= 0) and code or 1))
