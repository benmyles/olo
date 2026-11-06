-- Olo's touch filter: a libinput plugin app_server writes from
-- Preferences ▸ Touchpad and loads into its own libinput (see
-- crates/app_server/src/touch_filter.rs and docs/TOUCHPAD.md).
--
-- On pads that report each finger's pressure:
--  * a contact counts as a touch only while it presses (from `touch_on`
--    until it has been below `touch_off` for `lift_ms`), so a finger
--    hovering just above the glass is not a second finger;
--  * a contact as large as a palm (`palm_size`) is left out for as long
--    as it lasts, and so is one the person's palm model (`palm_model`,
--    trained in Preferences: crates/palm) scores as a palm in its first
--    `decide_ms` (from the same features, made the same way, as
--    `olo_palm::Tracker`);
--  * with a palm on the pad, the driver's click (made from the pressure of
--    everything touching) counts only if the fingers alone press hard
--    enough; without one it passes as it is. Olo never clicks when the
--    driver doesn't: the pad's haptic click is the driver's, and a click
--    nobody felt is worse than none;
--  * a touch's size is reported major-first (the driver can swap them,
--    which libinput's size-based touch detection would read as too small).
-- What is left out is reported with no size, so libinput sees it hover.
-- A frame never grows: libinput keeps the frame as the pad sent it when a
-- plugin returns more events than it was given (see `filter`).
--
-- libinput drops pressure on pads it takes for pressure pads: those with
-- INPUT_PROP_PRESSUREPAD (app_server's quirk takes it away) or a pressure
-- axis with a resolution (cleared below, before libinput looks). Until a
-- pad has sent a pressure, its frames pass through untouched: without
-- it every touch would look like a hover and the pad would go dead.

local CONFIG = {} -- @CONFIG@ (replaced by app_server)

-- When a key was last pressed on any keyboard (microseconds; palms rest
-- while typing), for the palm model.
-- The keyboard's keys: KEY_ESC up to the buttons (BTN_MISC, 0x100, which
-- libinput's evdev table doesn't name: an alias, so nil there).
local KEYS_END = evdev.KEY_ESC - 1 + 0x100
local last_key = nil

libinput:register({1})

local function wanted(device)
    for _, part in ipairs(CONFIG.devices) do
        if string.find(device:name(), part, 1, true) then
            return true
        end
    end
    return false
end

local function contact(state, n)
    local c = state.slots[n]
    if not c then
        c = { id = -1, pressure = 0, major = 0, minor = 0, x = 0, y = 0, orientation = 0, width = 0, on = false, palm = false, low_since = nil }
        state.slots[n] = c
    end
    return c
end

-- The palm model's features of contact `c` at `time` (microseconds),
-- `others` the other contacts down: as `olo_palm::Tracker::frame`, which
-- follows a contact from its first report on the glass (nil before: it
-- hovers).
local function features(state, c, time, others, nearest)
    local a = state.axes
    if not c.track and a.pressure_max > 0 and c.pressure <= 0 then
        return nil, 0
    end
    local w = math.max(a.x_max - a.x_min, 1)
    local h = math.max(a.y_max - a.y_min, 1)
    local x = (c.x - a.x_min) / w
    local y = (c.y - a.y_min) / h
    local size = math.max(c.major, c.minor) / math.max(a.size_max, 1)
    local small = math.min(c.major, c.minor) / math.max(a.size_max, 1)
    local pressure = c.pressure / math.max(a.pressure_max, 1)
    local t = c.track
    if not t then
        t = { down = time, x = x, y = y, at = time, speed = 0, peak_size = 0, peak_pressure = 0, first_size = size }
        c.track = t
    end
    local dt = (time - t.at) / 1e6
    if dt > 0 then
        local moved = math.sqrt((x - t.x) ^ 2 + (y - t.y) ^ 2) / dt
        t.speed = 0.7 * t.speed + 0.3 * moved
    end
    t.x, t.y, t.at = x, y, time
    t.peak_size = math.max(t.peak_size, size)
    t.peak_pressure = math.max(t.peak_pressure, pressure)
    local age_ms = (time - t.down) // 1000
    return {
        size,
        small,
        pressure,
        pressure / (size + 0.01),
        x,
        y,
        math.min(x, 1 - x, y, 1 - y),
        math.min(age_ms / 1000, 1),
        math.min(t.speed, 5) / 5,
        t.peak_size,
        t.peak_pressure,
        math.min(others, 4) / 4,
        small / (size + 0.01),
        math.min(size / (t.first_size + 0.01), 3) / 3,
        nearest,
        a.orientation_max > 0 and math.max(-1, math.min(1, c.orientation / a.orientation_max)) or 0,
        math.min(c.width / math.max(c.major, 1), 2) / 2,
        last_key and math.min((time - last_key) / 1e6, 1) or 1,
    }, age_ms
end

-- The palm model's score for features `f`: as `olo_palm::score`.
local function palm_score(m, f)
    -- (Held within what it was trained on, when it says.)
    if #m.lo == m.features then
        for i = 1, m.features do
            f[i] = math.min(math.max(f[i], m.lo[i]), m.hi[i])
        end
    end
    local out = m.b2
    for j = 1, m.hidden do
        local a = m.b1[j]
        for i = 1, m.features do
            a = a + m.w1[(j - 1) * m.features + i] * ((f[i] - m.mean[i]) / m.std[i])
        end
        if a > 0 then
            out = out + m.w2[j] * a
        end
    end
    return out
end

-- What libinput was last given for slot `n`'s size (nil: not known).
local function given(state, n)
    local g = state.given[n]
    if not g then
        g = {}
        state.given[n] = g
    end
    return g
end

local function filter(state, frame, time)
    if not state.pressure then
        for _, e in ipairs(frame) do
            if e.usage == evdev.ABS_MT_PRESSURE then
                state.pressure = true
                libinput:log_info("olo: pressure seen, filtering touches")
            end
        end
        if not state.pressure then
            return nil
        end
    end

    -- What the frame says, slot by slot (and which slots' sizes it sets).
    local slot = state.slot
    local sized_major, sized_minor = {}, {}
    for _, e in ipairs(frame) do
        local u, v = e.usage, e.value
        if u == evdev.ABS_MT_SLOT then
            slot = v
        elseif u == evdev.ABS_MT_TRACKING_ID then
            local c = contact(state, slot)
            c.id = v
            if v >= 0 then
                c.pressure, c.major, c.minor = 0, 0, 0
                c.on, c.palm, c.low_since, c.track, c.decided = false, false, nil, nil, false
            end
        elseif u == evdev.ABS_MT_PRESSURE then
            contact(state, slot).pressure = v
        elseif u == evdev.ABS_MT_TOUCH_MAJOR then
            contact(state, slot).major = v
            sized_major[slot] = true
        elseif u == evdev.ABS_MT_TOUCH_MINOR then
            contact(state, slot).minor = v
            sized_minor[slot] = true
        elseif u == evdev.ABS_MT_POSITION_X then
            contact(state, slot).x = v
        elseif u == evdev.ABS_MT_POSITION_Y then
            contact(state, slot).y = v
        elseif u == evdev.ABS_MT_ORIENTATION then
            contact(state, slot).orientation = v
        elseif u == evdev.ABS_MT_WIDTH_MAJOR then
            contact(state, slot).width = v
        elseif u == evdev.BTN_LEFT and CONFIG.click then
            -- The driver's click: passed on below, unless a palm made it.
            state.driver = v == 1
        end
    end
    local last_slot = slot

    -- Each contact: does it count, and so the size libinput should see.
    local want = {}
    local fingers, palms = 0, 0
    local down = 0
    for _, c in pairs(state.slots) do
        if c.id >= 0 then
            down = down + 1
        end
    end
    local model = CONFIG.palm_model
    for n, c in pairs(state.slots) do
        if c.id >= 0 then
            if math.max(c.major, c.minor) >= CONFIG.palm_size then
                c.palm = true
            end
            -- (Judged in its first `decide_ms`; after, nothing to do.)
            if model and state.axes and not c.palm and not c.decided then
                -- The nearest other contact, in pad widths (1: none).
                local nearest = 1
                local w = math.max(state.axes.x_max - state.axes.x_min, 1)
                for _, o in pairs(state.slots) do
                    if o.id >= 0 and o ~= c then
                        nearest = math.min(nearest, math.sqrt(((o.x - c.x) / w) ^ 2 + ((o.y - c.y) / w) ^ 2))
                    end
                end
                local f, age_ms = features(state, c, time, down - 1, nearest)
                if not f then
                    -- (Hovering: judged once it touches.)
                elseif age_ms > model.decide_ms then
                    c.decided = true
                elseif age_ms >= (model.min_age_ms or 0) and palm_score(model, f) > model.threshold then
                    c.palm = true
                end
            end
            if not c.on then
                if c.pressure >= CONFIG.touch_on then
                    c.on, c.low_since = true, nil
                end
            elseif c.pressure < CONFIG.touch_off then
                c.low_since = c.low_since or time
                if time - c.low_since >= CONFIG.lift_ms * 1000 then
                    c.on = false
                end
            else
                c.low_since = nil
            end
            if c.on and not c.palm then
                want[n] = { math.max(c.major, c.minor), math.min(c.major, c.minor) }
            else
                want[n] = { 0, 0 }
            end
            if c.palm then
                palms = palms + 1
            else
                fingers = fingers + c.pressure
            end
        end
    end

    local click = nil
    if CONFIG.click then
        local fingers_press = palms == 0 or fingers >= CONFIG.press
        local fingers_let_go = palms > 0 and fingers <= CONFIG.release
        if not state.button and state.driver and fingers_press then
            click = 1
        elseif state.button and (not state.driver or fingers_let_go) then
            click = 0
        end
    end

    -- libinput takes back no more events than it gave (a frame it held
    -- back is a copy with no room to spare: given more, it drops what the
    -- plugin made and keeps the frame as the pad sent it, and the sizes
    -- the filter thought it had sent are never seen). So: the sizes the
    -- frame carries are written as they should be where they stand,
    -- pressure (which libinput doesn't use here) and the driver's click
    -- make room, and a size the frame doesn't carry goes in only if there
    -- is room, else with a later frame.
    local base = 0
    for _, e in ipairs(frame) do
        local u = e.usage
        if u ~= evdev.ABS_MT_PRESSURE and u ~= evdev.ABS_PRESSURE and not (u == evdev.BTN_LEFT and CONFIG.click) then
            base = base + 1
        end
    end
    local room = #frame - base
    if click ~= nil then
        if room >= 1 then
            room = room - 1
        else
            click = nil -- next frame
        end
    end
    -- What's to be fixed for each slot: in its own part of the frame (the
    -- part before any slot event belongs to the slot the last frame
    -- ended in), or added after it, with a slot event, and the slot put
    -- back after.
    local present = { [state.slot] = true }
    for _, e in ipairs(frame) do
        if e.usage == evdev.ABS_MT_SLOT then
            present[e.value] = true
        end
    end
    -- Major first: a contact with no major (below the touch size's
    -- low end) is no touch to libinput, whatever its minor.
    local take = {}
    local slots = {}
    for n in pairs(want) do
        table.insert(slots, n)
    end
    table.sort(slots)
    local function needs(n)
        local g, w = given(state, n), want[n]
        return g.major ~= w[1] and not sized_major[n], g.minor ~= w[2] and not sized_minor[n]
    end
    for _, n in ipairs(slots) do
        local major, minor = needs(n)
        if present[n] and (major or minor) then
            local t = { major = major and room >= 1 }
            if t.major then room = room - 1 end
            t.minor = minor and room >= 1
            if t.minor then room = room - 1 end
            take[n] = t
        end
    end
    local restore = false
    local after = {}
    for _, n in ipairs(slots) do
        local major, minor = needs(n)
        if not present[n] and (major or minor) then
            local back = ((not restore) and n ~= last_slot) and 1 or 0
            if room >= 2 + back then
                room = room - 1 - back
                restore = restore or n ~= last_slot
                local t = { major = major and room >= 1 }
                if t.major then room = room - 1 end
                t.minor = minor and room >= 1
                if t.minor then room = room - 1 end
                take[n] = t
                table.insert(after, n)
            end
        end
    end

    local out = {}
    local function fix(n)
        local g, w, t = given(state, n), want[n], take[n]
        if not (w and t) then
            return
        end
        if t.major then
            table.insert(out, { usage = evdev.ABS_MT_TOUCH_MAJOR, value = w[1] })
            g.major = w[1]
        end
        if t.minor then
            table.insert(out, { usage = evdev.ABS_MT_TOUCH_MINOR, value = w[2] })
            g.minor = w[2]
        end
        take[n] = nil
    end
    local cur = state.slot
    for _, e in ipairs(frame) do
        local u, v = e.usage, e.value
        if u == evdev.ABS_MT_SLOT then
            if present[cur] then
                fix(cur)
            end
            cur = v
            table.insert(out, e)
        elseif u == evdev.ABS_MT_TOUCH_MAJOR then
            local w = want[cur]
            local value = w and w[1] or v
            given(state, cur).major = value
            table.insert(out, { usage = u, value = value })
        elseif u == evdev.ABS_MT_TOUCH_MINOR then
            local w = want[cur]
            local value = w and w[2] or v
            given(state, cur).minor = value
            table.insert(out, { usage = u, value = value })
        elseif u == evdev.ABS_MT_PRESSURE or u == evdev.ABS_PRESSURE then
            -- (Kept by the filter; libinput has no use for it here.)
        elseif u == evdev.BTN_LEFT and CONFIG.click then
            -- (The filter's own click goes below.)
        else
            table.insert(out, e)
        end
    end
    fix(cur)
    for _, n in ipairs(after) do
        table.insert(out, { usage = evdev.ABS_MT_SLOT, value = n })
        fix(n)
    end
    if restore then
        -- Back to the slot the next frame from the kernel carries on in.
        table.insert(out, { usage = evdev.ABS_MT_SLOT, value = last_slot })
    end
    state.slot = last_slot
    if click ~= nil then
        state.button = click == 1
        table.insert(out, { usage = evdev.BTN_LEFT, value = click })
    end
    return out
end

-- ---- A ThinkPad's pad (CONFIG.thinkpads: the Goodix GXTP5420) ----
--
-- Its firmware marks palms itself (ABS_MT_TOOL_TYPE = MT_TOOL_PALM), from
-- a palm's first report, but calls a resting palm a finger again on a
-- fifth to two fifths of its reports, and libinput follows the flag both
-- ways (a palm counts as a finger then); and the kernel ends a palm right
-- after reporting it, which comes back at the same place under a new id,
-- sometimes a finger from then on. So here a contact is a palm once the
-- firmware calls it one (and stays one until it lifts, across the
-- kernel's new ids), or once its palm model (`thinkpad_model`, made from
-- real hands on this pad: crates/palm's `Kind::ThinkPad`, the same
-- features as `olo_palm::Tracker` makes) says so in its first
-- `decide_ms`; and libinput is told so by its tool type (it ignores a
-- palm). The pad reports no size and no hover, and its click is its own
-- button, so nothing else is changed. Room for a tool type the frame
-- doesn't carry is made from what libinput doesn't read on such a pad
-- (the single-touch ABS_X, ABS_Y and ABS_PRESSURE, and ABS_MT_PRESSURE,
-- which app_server's quirk tells libinput not to use).

local MT_TOOL_PALM = 2
-- A palm that ends and a contact that lands this soon (microseconds) and
-- this near (millimetres) are one contact (as `olo_palm::AGAIN_US`).
local AGAIN_US = 40000
local AGAIN_MM = 5

local function thinkpad_wanted(device)
    for _, part in ipairs(CONFIG.thinkpads or {}) do
        if string.find(device:name(), part, 1, true) then
            return true
        end
    end
    return false
end

local function tp_contact(state, n)
    local c = state.slots[n]
    if not c then
        c = { id = -1, x = 0, y = 0, pressure = 0, tool = 0, palm = false, decided = false, track = nil }
        state.slots[n] = c
    end
    return c
end

-- Contact `c`'s features at `time` (as `olo_palm::kind::thinkpad_features`):
-- what the firmware says of it (now, ever, how often, at its first
-- report), its speed, its force now and at most, and its age.
local function tp_features(state, c, time)
    local a = state.axes
    local x = (c.x - a.x_min) / math.max(a.x_max - a.x_min, 1)
    local y = (c.y - a.y_min) / math.max(a.y_max - a.y_min, 1)
    local pressure = c.pressure / math.max(a.pressure_max, 1)
    local palm = c.tool == MT_TOOL_PALM
    local t = c.track
    if not t then
        t = { down = time, x = x, y = y, at = time, speed = 0, reports = 0, palms = 0, first = palm, peak = 0 }
        c.track = t
    end
    local dt = (time - t.at) / 1e6
    if dt > 0 then
        local moved = math.sqrt((x - t.x) ^ 2 + (y - t.y) ^ 2) / dt
        t.speed = 0.7 * t.speed + 0.3 * moved
    end
    t.x, t.y, t.at = x, y, time
    if t.reports == 0 then
        t.first = palm
    end
    t.reports = t.reports + 1
    if palm then
        t.palms = t.palms + 1
    end
    t.peak = math.max(t.peak, pressure)
    local age_ms = (time - t.down) // 1000
    return {
        palm and 1 or 0,
        t.palms > 0 and 1 or 0,
        t.palms / t.reports,
        t.first and 1 or 0,
        math.min(t.speed, 5) / 5,
        pressure,
        t.peak,
        math.min(age_ms / 1000, 1),
    }, age_ms
end

local function filter_thinkpad(state, frame, time)
    -- What the frame says, slot by slot, and which contacts start in it.
    local slot = state.slot
    local started = {}
    for _, e in ipairs(frame) do
        local u, v = e.usage, e.value
        if u == evdev.ABS_MT_SLOT then
            slot = v
        elseif u == evdev.ABS_MT_TRACKING_ID then
            local c = tp_contact(state, slot)
            if v < 0 and c.id >= 0 and c.track and c.track.palms > 0 then
                -- A palm ending: kept a moment, for it to come back.
                table.insert(state.ended, { at = time, x = c.x, y = c.y, track = c.track, palm = c.palm })
            end
            c.id = v
            if v >= 0 then
                c.palm, c.decided, c.track = false, false, nil
                started[slot] = true
            end
        elseif u == evdev.ABS_MT_POSITION_X then
            tp_contact(state, slot).x = v
        elseif u == evdev.ABS_MT_POSITION_Y then
            tp_contact(state, slot).y = v
        elseif u == evdev.ABS_MT_PRESSURE then
            tp_contact(state, slot).pressure = v
        elseif u == evdev.ABS_MT_TOOL_TYPE then
            tp_contact(state, slot).tool = v
        end
    end
    local last_slot = slot

    local kept = {}
    for _, e in ipairs(state.ended) do
        if time - e.at <= AGAIN_US then
            table.insert(kept, e)
        end
    end
    state.ended = kept
    local a = state.axes
    local model = CONFIG.thinkpad_model
    local want = {}
    for n, c in pairs(state.slots) do
        if c.id >= 0 then
            -- A palm the kernel ended a moment ago, back under a new id.
            if started[n] and #state.ended > 0 then
                for i, e in ipairs(state.ended) do
                    local dx = (c.x - e.x) / math.max(a.x_res, 1)
                    local dy = (c.y - e.y) / math.max(a.y_res, 1)
                    if math.sqrt(dx * dx + dy * dy) <= AGAIN_MM then
                        c.track, c.palm = e.track, e.palm
                        table.remove(state.ended, i)
                        break
                    end
                end
            end
            local f, age_ms = tp_features(state, c, time)
            if c.tool == MT_TOOL_PALM or c.track.palms > 0 then
                c.palm = true
            elseif model and not c.palm and not c.decided then
                if age_ms > model.decide_ms then
                    c.decided = true
                elseif age_ms >= (model.min_age_ms or 0) and palm_score(model, f) > model.threshold then
                    c.palm = true
                end
            end
            want[n] = c.palm and MT_TOOL_PALM or 0
        end
    end

    -- libinput takes back no more events than it gave: the tool types
    -- the frame carries are written as they should be where they stand;
    -- one it doesn't carry takes the place of events libinput doesn't
    -- read here, in its slot's part of the frame, or after it with a slot
    -- event (and the slot put back after), else waits for a later frame.
    local function idle(u)
        return u == evdev.ABS_X or u == evdev.ABS_Y or u == evdev.ABS_PRESSURE or u == evdev.ABS_MT_PRESSURE
    end
    local room = 0
    local present = { [state.slot] = true }
    local carried = {}
    local rewrite = false
    local cur = state.slot
    for _, e in ipairs(frame) do
        if idle(e.usage) then
            room = room + 1
        elseif e.usage == evdev.ABS_MT_SLOT then
            cur = e.value
            present[cur] = true
        elseif e.usage == evdev.ABS_MT_TOOL_TYPE then
            carried[cur] = true
            rewrite = rewrite or (want[cur] ~= nil and want[cur] ~= e.value)
        end
    end
    local function needs(n)
        return want[n] ~= nil and not carried[n] and (state.given[n] or 0) ~= want[n]
    end
    local missing = false
    for n in pairs(want) do
        missing = missing or needs(n)
    end
    if not rewrite and not (missing and room > 0) then
        -- (Nothing to change: what libinput is told passes as it came.)
        cur = state.slot
        for _, e in ipairs(frame) do
            if e.usage == evdev.ABS_MT_SLOT then
                cur = e.value
            elseif e.usage == evdev.ABS_MT_TOOL_TYPE then
                state.given[cur] = e.value
            end
        end
        state.slot = last_slot
        return nil
    end
    local out = {}
    local function fix(n)
        if present[n] and needs(n) and room >= 1 then
            room = room - 1
            table.insert(out, { usage = evdev.ABS_MT_TOOL_TYPE, value = want[n] })
            state.given[n] = want[n]
        end
    end
    cur = state.slot
    for _, e in ipairs(frame) do
        local u, v = e.usage, e.value
        if u == evdev.ABS_MT_SLOT then
            fix(cur)
            cur = v
            table.insert(out, e)
        elseif u == evdev.ABS_MT_TOOL_TYPE then
            local value = want[cur] or v
            state.given[cur] = value
            table.insert(out, { usage = u, value = value })
        elseif not idle(u) then
            table.insert(out, e)
        end
    end
    fix(cur)
    -- Slots the frame doesn't touch: their slot, the tool, and once the
    -- slot the next frame carries on in.
    local slots = {}
    for n in pairs(want) do
        if not present[n] and needs(n) then
            table.insert(slots, n)
        end
    end
    table.sort(slots)
    local back = false
    for _, n in ipairs(slots) do
        local cost = back and 2 or 3
        if room >= cost then
            room = room - 2
            if not back then
                room = room - 1
                back = true
            end
            table.insert(out, { usage = evdev.ABS_MT_SLOT, value = n })
            table.insert(out, { usage = evdev.ABS_MT_TOOL_TYPE, value = want[n] })
            state.given[n] = want[n]
        end
    end
    if back then
        table.insert(out, { usage = evdev.ABS_MT_SLOT, value = last_slot })
    end
    state.slot = last_slot
    return out
end

libinput:connect("new-evdev-device", function(device)
    local usages = device:usages()
    -- A keyboard: its key presses noted, nothing changed.
    if usages[evdev.KEY_A] or usages[evdev.KEY_SPACE] then
        device:connect("evdev-frame", function(_, frame, time)
            for _, e in ipairs(frame) do
                if e.value == 1 and e.usage >= evdev.KEY_ESC and e.usage < KEYS_END then
                    last_key = time
                end
            end
            return nil
        end)
    end
    if thinkpad_wanted(device) and usages[evdev.ABS_MT_TOOL_TYPE] and usages[evdev.ABS_MT_POSITION_X] then
        if usages[evdev.ABS_MT_PRESSURE] then
            device:set_absinfo(evdev.ABS_MT_PRESSURE, { resolution = 0 })
        end
        local abs = device:absinfos()
        local ax, ay, ap = abs[evdev.ABS_MT_POSITION_X], abs[evdev.ABS_MT_POSITION_Y], abs[evdev.ABS_MT_PRESSURE]
        local state = {
            slot = 0, slots = {}, given = {}, ended = {},
            axes = { x_min = ax.minimum, x_max = ax.maximum, y_min = ay.minimum, y_max = ay.maximum,
                     x_res = ax.resolution or 0, y_res = ay.resolution or 0, pressure_max = ap and ap.maximum or 0 },
        }
        libinput:log_info("olo: keeping palms palms on " .. device:name())
        device:connect("evdev-frame", function(_, frame, time)
            return filter_thinkpad(state, frame, time)
        end)
        return
    end
    if not wanted(device) then
        return
    end
    if not (usages[evdev.ABS_MT_PRESSURE] and usages[evdev.ABS_MT_TOUCH_MAJOR] and usages[evdev.ABS_MT_TOUCH_MINOR]) then
        return
    end
    device:set_absinfo(evdev.ABS_MT_PRESSURE, { resolution = 0 })
    if device:absinfos()[evdev.ABS_PRESSURE] then
        device:set_absinfo(evdev.ABS_PRESSURE, { resolution = 0 })
    end
    libinput:log_info("olo: filtering touches on " .. device:name())
    local state = { slot = 0, slots = {}, given = {}, button = false, driver = false, pressure = false }
    -- The pad's axes, for the palm model's features.
    local abs = device:absinfos()
    local ax, ay, am, ap = abs[evdev.ABS_MT_POSITION_X], abs[evdev.ABS_MT_POSITION_Y], abs[evdev.ABS_MT_TOUCH_MAJOR], abs[evdev.ABS_MT_PRESSURE]
    if ax and ay and am and ap then
        local ao = abs[evdev.ABS_MT_ORIENTATION]
        state.axes = { x_min = ax.minimum, x_max = ax.maximum, y_min = ay.minimum, y_max = ay.maximum, size_max = am.maximum, pressure_max = ap.maximum, orientation_max = ao and ao.maximum or 0 }
    end
    device:connect("evdev-frame", function(_, frame, time)
        return filter(state, frame, time)
    end)
end)
