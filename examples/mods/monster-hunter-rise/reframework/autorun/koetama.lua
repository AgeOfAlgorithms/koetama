--[[ Koetama Hunter Talk: Monster Hunter Rise (REFramework Lua), linked to Koetama by FILES (PROTOCOL.md
"Transport: files"; profile examples/profiles/monster-hunter-rise-koetama.json).

REFramework scripts can only read and write files under <game>/reframework/data/, so:
  - the feed is reframework/data/koetama/feed.json (json.dump_file), written 5 times a second and at once when the
    talk key changes, a line is queued for translation or a message was read;
  - Koetama's messages are reframework/data/koetama/kt_t<n>.json; the next one (n = ack + 1) is looked for every
    frame with fs.read (it answers "" for a missing file - json.load_file would log an error for each miss), then
    acked in the feed and Koetama deletes it;
  - kt_on says Koetama runs, kt_p<ping % 1000> answers our ping (every 2 s): no answer for 5 s = Koetama is gone.

Shows the player's speech (live words, then the line) and translations in an overlay window; the settings are in
REFramework's menu (Insert) under "Koetama". Push to talk: hold a keyboard key (V by default; REFramework reads it
with reframework:is_key_down, a Windows virtual-key code).

Other scripts can ask for a translation: Koetama.translate("texto", "who said it"). Nothing in MH Rise's chat is
hooked: the game's chat types are not documented, see the report. What lines are translated into is the player's
setting in Koetama's window ("Translate chat into"); this script only shows it (translations_status's "into").

Install: REFramework for MH Rise, then copy this file to <MonsterHunterRise>/reframework/autorun/. Start the game
once (this creates reframework/data/koetama), then start Koetama (its files connector only writes into folders that
exist when it starts) and pick Monster Hunter Rise.
Written for Lua 5.4 but kept to what LuaJIT also runs (the test harness): no //, no integer-only library calls.
]]

local DIR = "koetama/"
local FEED = DIR .. "feed.json"
local SETTINGS = DIR .. "settings.json"
local PREFIX = "kt_"
local FEED_EVERY = 0.2      -- s between feeds (Koetama counts the game gone after 1.5 s without a new one)
local PING_EVERY = 2.0
local GONE_AFTER = 5.0      -- s without a ping answer
local LINE_TTL = 10         -- s a line waits for its translation
local SHOW_FOR = 20         -- s a line stays in the overlay
local MAX_READ = 32         -- messages read in one frame at most

local cfg = {
    listen = "push_to_talk",  -- "push_to_talk", "always", "off"
    talk_vk = 0x56,           -- V
    lang = "en",
    live = true,
    overlay = true,
    game_chat = false,        -- also post lines into the game's chat log (unverified game call)
}

local kt = {
    session = 0, seq = 0, ack = 0, ping = 0,
    ping_at = 0, answered_at = nil,  -- when the current ping went out / was last answered
    next_feed = 0, dirty = false,
    talk_key = false,
    online = false, features = {}, version = nil,
    into = nil,              -- what Koetama translates into ("": off; nil: not said yet)
    states = {}, progress = {}, lines = {}, retry = {}, next_id = 1,  -- states/progress: "from>to" -> ..., the pairs in use
    shown = {},              -- {text=, kind="said"|"live"|"line"|"translation"|"info", at=}
    live = nil,              -- the words so far of the line being said
    started = nil,
}
Koetama = {state = kt, config = cfg}

local function now() return os.clock() end

-- ------------------------------------------------------------------ helpers
local function show(kind, text)
    kt.shown[#kt.shown + 1] = {kind = kind, text = text, at = now()}
    while #kt.shown > 30 do table.remove(kt.shown, 1) end
end

local function game_chat(text)
    if not cfg.game_chat or not sdk then return end
    -- (unverified: a system message in the player's own chat log, as some MH Rise mods do)
    pcall(function()
        local cm = sdk.get_managed_singleton("snow.gui.ChatManager")
        if cm then cm:call("reqAddChatInfomation", text, 0) end
    end)
end

local function cut_utf8(s, n)
    if #s <= n then return s end
    local i = n
    while i > 0 do
        local b = s:byte(i + 1)
        if not b or b < 0x80 or b >= 0xC0 then break end
        i = i - 1
    end
    return s:sub(1, i)
end

-- No translation is downloading or loading (Koetama answers "" for a line that meets one).
local function all_ready()
    for _, state in pairs(kt.states) do
        if state == "downloading" or state == "loading" then return false end
    end
    return true
end

-- ------------------------------------------------------------------ the feed
local function write_feed()
    kt.seq = kt.seq + 1
    local lines = {}
    local t = now()
    for id, line in pairs(kt.lines) do
        if t - line.at > LINE_TTL then kt.lines[id] = nil end
    end
    for id, line in pairs(kt.lines) do
        if #lines < 16 then lines[#lines + 1] = {id = id, text = line.text} end
    end
    table.sort(lines, function(a, b) return a.id < b.id end)
    local feed = {
        type = "feed", seq = kt.seq, session = kt.session, ack = kt.ack, ping = kt.ping,
        listen = cfg.listen, talk_key = kt.talk_key, lang = cfg.lang, live = cfg.live,
        to_translate = lines,
    }
    -- (an empty list is written as null by REFramework's json: Koetama reads null as "none", which is the same)
    json.dump_file(FEED, feed, -1)
    kt.next_feed = t + FEED_EVERY
    kt.dirty = false
end

local function queue_line(text, who)
    local id = kt.next_id
    kt.next_id = kt.next_id + 1
    kt.lines[id] = {text = cut_utf8(text, 400), who = who, at = now()}
    kt.dirty = true
    return id
end

function Koetama.translate(text, who)
    if kt.into == "" or text == nil or text == "" then return nil end -- (off in Koetama's window)
    return queue_line(text, who)
end

-- ------------------------------------------------------------------ Koetama's messages
local function on_object(o)
    local t = o.type
    if t == "hello" then
        kt.features = {}
        for _, f in ipairs(o.features or {}) do kt.features[f] = true end
        kt.version = o.version
        show("info", "Koetama " .. tostring(o.version) .. " linked")
    elseif t == "speech" then
        if o.kind == "start" then
            kt.live = ""
        elseif o.kind == "live" then
            kt.live = o.text
        elseif o.kind == "final" then
            kt.live = nil
            if o.text and o.text ~= "" then
                show("said", o.text)
                game_chat(o.text)
            end
        end
    elseif t == "translation" then
        local line = kt.lines[o.id]
        kt.lines[o.id] = nil
        if not line then return end
        if o.text ~= "" then
            show("translation", (line.who and (line.who .. ": ") or "") .. o.text)
            game_chat(o.text)
        elseif not all_ready() and not line.retried then
            line.retried = true
            kt.retry[#kt.retry + 1] = line  -- ("" while a model is not ready: again once it is)
        end
    elseif t == "translations_status" then
        if type(o.into) == "string" and o.into ~= kt.into then
            kt.into = o.into
            show("info", o.into == "" and "translation is off: choose a language in Koetama's window"
                or "Koetama translates chat into " .. o.into)
        end
        local was = kt.states
        kt.states, kt.progress = {}, {}
        for _, s in ipairs(o.translations or {}) do
            local key = tostring(s.from) .. ">" .. tostring(s.to)
            if was[key] ~= s.state then
                show("info", "translation " .. key:gsub(">", " > ") .. ": " .. tostring(s.state))
            end
            kt.states[key] = s.state
            kt.progress[key] = s.progress
        end
        if all_ready() and #kt.retry > 0 then
            for _, line in ipairs(kt.retry) do
                local id = queue_line(line.text, line.who)
                kt.lines[id].retried = true
            end
            kt.retry = {}
        end
    end
    -- room / voice: not used (the profile has no "voices"); see the report
end

-- The next message files, in order: kt_t<ack+1>.json, ...
local function read_messages()
    for _ = 1, MAX_READ do
        local n = kt.ack + 1
        local s = fs.read(DIR .. PREFIX .. "t" .. n .. ".json")
        if s == nil or s == "" then return end
        local o = json.load_string(s)
        if type(o) ~= "table" then return end -- (never half-written: Koetama renames it into place)
        kt.ack = n
        kt.dirty = true
        on_object(o)
    end
end

local function check_alive(t)
    if fs.read(DIR .. PREFIX .. "p" .. (kt.ping % 1000)) ~= "" then kt.answered_at = t end
    local on = fs.read(DIR .. PREFIX .. "on") ~= ""
    kt.online = on and kt.answered_at ~= nil and t - kt.answered_at < GONE_AFTER
end

-- ------------------------------------------------------------------ settings
local function save_settings() json.dump_file(SETTINGS, cfg) end

local function load_settings()
    -- (fs.read first: json.load_file logs an error when the file is not there yet)
    local s = fs.read(SETTINGS)
    local saved = s ~= "" and json.load_string(s) or nil
    if type(saved) == "table" then
        for k, v in pairs(saved) do if cfg[k] ~= nil then cfg[k] = v end end
    end
end

-- ------------------------------------------------------------------ every frame
local function frame()
    local t = now()
    if not kt.started then
        kt.started = t
        load_settings()
        math.randomseed(os.time())
        kt.session = os.time() * 1000 + math.random(0, 999)
        kt.ping_at = t
    end
    local down = cfg.listen == "push_to_talk" and reframework:is_key_down(cfg.talk_vk) or false
    if down ~= kt.talk_key then
        kt.talk_key = down
        kt.dirty = true
    end
    read_messages()
    if t - kt.ping_at >= PING_EVERY then
        kt.ping = kt.ping + 1
        kt.ping_at = t
        kt.dirty = true
    end
    check_alive(t)
    if kt.dirty or t >= kt.next_feed then write_feed() end
end

local COLORS = {said = 0xFFFFFFFF, live = 0xFFAAAAAA, translation = 0xFFFFD9A6, info = 0xFF909090}

local function draw_overlay()
    if not cfg.overlay or not imgui.begin_window then return end
    local t = now()
    if imgui.begin_window("Koetama", true, 0) then
        if not kt.online then
            imgui.text_colored(fs.read(DIR .. PREFIX .. "on") ~= "" and "Koetama is not answering"
                or "Koetama is not running (start it, then pick Monster Hunter Rise)", 0xFF6060FF)
        elseif cfg.listen == "push_to_talk" then
            imgui.text_colored(kt.talk_key and "talking" or "hold the talk key to speak", 0xFF909090)
        end
        for _, l in ipairs(kt.shown) do
            if t - l.at < SHOW_FOR then imgui.text_colored(l.text, COLORS[l.kind] or 0xFFFFFFFF) end
        end
        if kt.live and kt.live ~= "" then imgui.text_colored(kt.live .. " ...", COLORS.live) end
        imgui.end_window()
    end
end

local LISTEN = {"push_to_talk", "always", "off"}
local translate_box = ""

local function draw_settings()
    if not imgui.tree_node("Koetama") then return end
    local changed, v
    local idx = 1
    for i, l in ipairs(LISTEN) do if l == cfg.listen then idx = i end end
    changed, v = imgui.combo("Microphone", idx, LISTEN)
    if changed then cfg.listen = LISTEN[v]; kt.dirty = true; save_settings() end
    changed, v = imgui.input_text("Talk key (virtual-key code, hex)", string.format("%X", cfg.talk_vk))
    if changed and tonumber(v, 16) then cfg.talk_vk = tonumber(v, 16); save_settings() end
    changed, v = imgui.input_text("Language I speak", cfg.lang)
    if changed then cfg.lang = v; kt.dirty = true; save_settings() end
    -- translation: Koetama's own setting, shown here
    if kt.into == nil then
        imgui.text("Translation: not said yet (Koetama tells once it is linked)")
    elseif kt.into == "" then
        imgui.text("Translation is off: choose a language in Koetama's window")
    else
        imgui.text("Koetama translates chat into " .. kt.into)
    end
    local keys = {}
    for key in pairs(kt.states) do keys[#keys + 1] = key end
    table.sort(keys)
    for _, key in ipairs(keys) do
        local state = kt.states[key]
        if state == "downloading" and kt.progress[key] then
            state = string.format("downloading %d%%", math.floor(kt.progress[key] * 100))
        end
        imgui.text("  " .. key:gsub(">", " > ") .. ": " .. tostring(state))
    end
    imgui.text("Choose the language in Koetama's window (Translate chat into)")
    changed, v = imgui.input_text("Translate a line", translate_box)
    if changed then translate_box = v end
    if imgui.button("Translate") then Koetama.translate(translate_box, nil); translate_box = "" end
    changed, v = imgui.checkbox("Overlay", cfg.overlay)
    if changed then cfg.overlay = v; save_settings() end
    changed, v = imgui.checkbox("Also in the game's chat log (experimental)", cfg.game_chat)
    if changed then cfg.game_chat = v; save_settings() end
    imgui.text(string.format("session %d, message %d, ping %d, %s", kt.session, kt.ack, kt.ping,
        kt.online and "linked" or "not linked"))
    imgui.tree_pop()
end

re.on_frame(function()
    frame()
    draw_overlay()
end)
re.on_draw_ui(draw_settings)
