-- Olo's touch filter: a libinput plugin app_server writes from
-- Preferences ▸ Touchpad and loads into its own libinput (see
-- crates/app_server/src/touch_filter.rs and docs/TOUCHPAD.md).
--
-- On pads that report each finger's pressure:
--  * a contact counts as a touch only while it presses (from `touch_on`
--    until it has been below `touch_off` for `lift_ms`), so a finger
--    hovering just above the glass is not a second finger;
--  * a contact as large as a palm (`palm_size`) is left out for as long
--    as it lasts;
--  * the click comes from the pressure of the fingers alone (palms left
--    out), in place of the driver's, which sums everything touching;
--  * a touch's size is reported major-first (the driver can swap them,
--    which libinput's size-based touch detection would read as too small).
-- What is left out is reported with no size, so libinput sees it hover.

local CONFIG = {} -- @CONFIG@ (replaced by app_server)

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
        c = { id = -1, pressure = 0, major = 0, minor = 0, on = false, palm = false, low_since = nil, shown = nil }
        state.slots[n] = c
    end
    return c
end

local function filter(state, frame, time)
    local out = {}
    local slot = state.slot
    for _, e in ipairs(frame) do
        local u, v = e.usage, e.value
        if u == evdev.ABS_MT_SLOT then
            slot = v
            table.insert(out, e)
        elseif u == evdev.ABS_MT_TRACKING_ID then
            local c = contact(state, slot)
            c.id = v
            if v >= 0 then
                c.pressure, c.major, c.minor = 0, 0, 0
                c.on, c.palm, c.low_since, c.shown = false, false, nil, nil
            end
            table.insert(out, e)
        elseif u == evdev.ABS_MT_PRESSURE then
            contact(state, slot).pressure = v
            table.insert(out, e)
        elseif u == evdev.ABS_MT_TOUCH_MAJOR then
            contact(state, slot).major = v
        elseif u == evdev.ABS_MT_TOUCH_MINOR then
            contact(state, slot).minor = v
        elseif (u == evdev.BTN_LEFT) and CONFIG.click then
            -- The driver's click: made again below from the fingers.
        else
            table.insert(out, e)
        end
    end
    state.slot = slot

    local fingers = 0
    local moved = false
    for n, c in pairs(state.slots) do
        if c.id >= 0 then
            if math.max(c.major, c.minor) >= CONFIG.palm_size then
                c.palm = true
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
            local live = c.on and not c.palm
            local major, minor = 0, 0
            if live then
                major, minor = math.max(c.major, c.minor), math.min(c.major, c.minor)
            end
            local shown = major * 65536 + minor
            if c.shown ~= shown then
                table.insert(out, { usage = evdev.ABS_MT_SLOT, value = n })
                table.insert(out, { usage = evdev.ABS_MT_TOUCH_MAJOR, value = major })
                table.insert(out, { usage = evdev.ABS_MT_TOUCH_MINOR, value = minor })
                c.shown = shown
                moved = true
            end
            if not c.palm then
                fingers = fingers + c.pressure
            end
        end
    end
    if moved then
        -- Back to the slot the next frame from the kernel carries on in.
        table.insert(out, { usage = evdev.ABS_MT_SLOT, value = slot })
    end

    if CONFIG.click then
        if not state.button and fingers >= CONFIG.press then
            state.button = true
            table.insert(out, { usage = evdev.BTN_LEFT, value = 1 })
        elseif state.button and fingers <= CONFIG.release then
            state.button = false
            table.insert(out, { usage = evdev.BTN_LEFT, value = 0 })
        end
    end
    return out
end

libinput:connect("new-evdev-device", function(device)
    if not wanted(device) then
        return
    end
    local usages = device:usages()
    if not (usages[evdev.ABS_MT_PRESSURE] and usages[evdev.ABS_MT_TOUCH_MAJOR] and usages[evdev.ABS_MT_TOUCH_MINOR]) then
        return
    end
    libinput:log_info("olo: filtering touches on " .. device:name())
    local state = { slot = 0, slots = {}, button = false }
    device:connect("evdev-frame", function(_, frame, time)
        return filter(state, frame, time)
    end)
end)
