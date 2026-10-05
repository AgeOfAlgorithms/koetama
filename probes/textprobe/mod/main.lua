#version 2
-- Text probe (a test mod, not for the Workshop): can a helper program pass TEXT into the game? (For voice:
-- what was said, transcribed by the helper, into the chat history.) HasFile only says yes or no; this tries:
--   A. the helper writes a small prefab file while the level runs; the script Spawns it and reads the text
--      from what it made: the description (GetDescription: free text) and a tag (GetTagValue: hex, no spaces)
--   B. the helper writes an image; the script reads its size (UiGetImageSize): two numbers per file
-- and whether a file written again under the same name is read again or remembered. Run
-- probes/textprobe/probe.py first (it installs this mod and writes the files), then play any level with this
-- mod enabled; ~20 s. Results go to savegame.mod.txp (client side: in real use each machine reads its own
-- disk): r<k> for message file m<k>.xml, rup (a file one folder up), rs1 / rs2 (same.xml, then again after
-- it was rewritten), i1 (img1.png), is1 / is2 (imgsame.png, then again), g (which Lua file functions exist),
-- n = frame counter (written last). Text is written as hex so that anything survives the savegame.

local SLOTS = 20

local function hex(s)
	return (tostring(s):gsub(".", function(ch) return string.format("%02x", ch:byte()) end))
end

local function has(path)
	local ok, v = pcall(HasFile, path)
	return ok and v == true
end

-- Spawn the prefab file far below the level, read "ok:<entities>:<types>:<hex description>:<tag t>", delete it
local function readPrefab(path)
	local ok, ents = pcall(Spawn, path, Transform(Vec(0, -500, 0)), true)
	if not ok then return "err:" .. hex(tostring(ents):sub(1, 80)) end
	ents = ents or {}
	local desc, tag, types = "", "", {}
	for i = 1, #ents do
		local h = ents[i]
		local okT, ty = pcall(GetEntityType, h)
		types[#types + 1] = okT and tostring(ty) or "?"
		local okD, d = pcall(GetDescription, h)
		if okD and d and d ~= "" and desc == "" then desc = d end
		local okG, t = pcall(GetTagValue, h, "t")
		if okG and t and t ~= "" and tag == "" then tag = t end
	end
	for i = 1, #ents do pcall(Delete, ents[i]) end
	return "ok:" .. #ents .. ":" .. table.concat(types, ",") .. ":" .. hex(desc) .. ":" .. tag
end

function client.init()
	client.n = 0
	client.next = 1
	client.done = {}
	client.img = {}
	client.last = ""
	ClearKey("savegame.mod.txp")
	SetString("savegame.mod.txp.g", table.concat({type(dofile), type(loadfile), type(loadstring), type(io), type(os), type(require)}, ","))
end

local function once(key, cond, fn)
	if not client.done[key] and cond then
		client.done[key] = true
		local r = fn()
		SetString("savegame.mod.txp." .. key, r)
		client.last = key
	end
end

function client.tick(dt)
	local c = client
	c.n = c.n + 1
	while c.next <= SLOTS and has("MOD/sig/m" .. c.next .. ".xml") do
		local k = c.next
		once("r" .. k, true, function() return readPrefab("MOD/sig/m" .. k .. ".xml") end)
		c.next = k + 1
	end
	once("rup", has("MOD/../txp_up.xml"), function() return readPrefab("MOD/../txp_up.xml") end)
	once("rs1", has("MOD/sig/same.xml"), function() return readPrefab("MOD/sig/same.xml") end)
	once("rs2", c.done.rs1 and has("MOD/sig/same2.go"), function() return readPrefab("MOD/sig/same.xml") end)
	for key, v in pairs(c.img) do                                       -- (read in draw: Ui functions only work there)
		once(key, true, function() return v end)
	end
	SetInt("savegame.mod.txp.n", c.n)
end

local function imageSize(key, path, cond)
	if client.img[key] == nil and cond and has(path) then
		local ok, w, h = pcall(UiGetImageSize, path)
		client.img[key] = ok and (tostring(w) .. "x" .. tostring(h)) or "err"
	end
end

function client.draw()
	imageSize("i1", "MOD/sig/img1.png", true)
	imageSize("is1", "MOD/sig/imgsame.png", true)
	imageSize("is2", "MOD/sig/imgsame.png", client.done.is1 and has("MOD/sig/img2.go"))
	local line = "TEXT PROBE   files read: " .. (client.next - 1) .. "   last: " .. (client.last ~= "" and client.last or "-")
		.. "   (the probe program says when it is done)"
	UiPush()
	UiTranslate(UiCenter(), 40)
	UiAlign("center top")
	UiFont("bold.ttf", 24)
	local w = UiGetTextSize(line)
	UiColor(0, 0, 0, 0.6)
	UiRect(w + 40, 44)
	UiTranslate(0, 8)
	UiColor(1, 0.9, 0.4)
	UiText(line)
	UiPop()
end
