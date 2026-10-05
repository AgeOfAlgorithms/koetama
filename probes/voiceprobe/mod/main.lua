#version 2
#include "plan.lua"
-- Voice probe (a test mod, not for the Workshop): how does a voice sound when the GAME plays it from short
-- sound files a helper program writes while it is being spoken? probes/voiceprobe/probe.py (run it first)
-- writes MOD/sig/r<run>_<k>.ogg in real time, clip k when its audio "has been said"; this mod loads each
-- file the frame HasFile sees it and plays the clips on a clock that starts when the first one arrives:
--   clip k starts at t0 + k * L - lead, t0 = first clip seen + JB (the jitter buffer)
-- A frame is up to 17 ms late for that; where the file starts with `pre` seconds of silence the clip is
-- started that much further in (SetSoundProgress), so the clips line up to better than a frame.
-- To the probe, through savegame.mod.vp:  run / st ("wait", "play", "pause", "done"), play = "<run>.<k>"
-- when clip k starts, r<run> = "underruns,max late ms,mean late ms,clips played", n = frame counter (last).

local JB = 0.05
local PAUSE = 1.5

local function name(r, k) return "MOD/sig/r" .. r .. "_" .. k .. ".ogg" end

local function has(path)
	local ok, v = pcall(HasFile, path)
	return ok and v == true
end

local function startRun(c, r)
	c.r, c.st = r, "wait"
	c.loaded, c.nLoaded, c.k = {}, 0, 0
	c.t0, c.under, c.lateMax, c.lateSum, c.waiting = nil, 0, 0, 0, false
end

function client.init()
	client.n = 0
	client.unload = {}
	client.until_ = 0
	ClearKey("savegame.mod.vp")
	if #VP_RUNS == 0 then client.st = "done" else startRun(client, 1) end
end

function client.tick(dt)
	local c = client
	local now = GetTime()
	c.n = c.n + 1
	local run = VP_RUNS[c.r or 0]
	for i = #c.unload, 1, -1 do                                         -- (clips that have ended)
		if now >= c.unload[i][1] then
			pcall(UnloadSound, c.unload[i][2])
			table.remove(c.unload, i)
		end
	end
	if c.st == "pause" and now >= c.until_ then
		if c.r < #VP_RUNS then startRun(c, c.r + 1) else c.st = "done" end
		run = VP_RUNS[c.r]
	end
	if run and (c.st == "wait" or c.st == "play") then
		while c.nLoaded < run.n and has(name(c.r, c.nLoaded)) do        -- (load a clip the frame it is there)
			local ok, h = pcall(LoadSound, name(c.r, c.nLoaded))
			c.loaded[c.nLoaded] = ok and h or false
			c.nLoaded = c.nLoaded + 1
		end
		if c.st == "wait" and c.nLoaded > 0 then
			c.st, c.t0 = "play", now + JB + run.lead
			local cam = GetCameraTransform()                            -- (the spatial run's "speaker": 4 m ahead)
			local fwd = TransformToParentVec(cam, Vec(0, 0, -1))
			fwd[2] = 0
			c.point = VecAdd(cam.pos, VecScale(VecNormalize(fwd), 4))
		end
		while c.st == "play" and c.k < run.n do
			local due = c.t0 + c.k * run.L - run.lead
			if now < due then break end
			local h = c.loaded[c.k]
			if h == nil then                                            -- (not here yet: hold the clock)
				if not c.waiting then c.under, c.waiting = c.under + 1, true end
				c.t0 = c.t0 + (now - due)
				break
			end
			c.waiting = false
			local late = now - due
			if h then
				local pos = run.spatial and c.point or GetCameraTransform().pos
				local ok, play = pcall(PlaySound, h, pos, 1.0, false)
				if ok and play and run.pre > 0 and late > 0 then
					pcall(SetSoundProgress, play, math.min(late, run.pre))
				end
				c.unload[#c.unload + 1] = {due + run.L + 2 * run.lead + 1, h}
			end
			c.lateMax = math.max(c.lateMax, late)
			c.lateSum = c.lateSum + late
			SetString("savegame.mod.vp.play", c.r .. "." .. c.k)
			c.k = c.k + 1
		end
		if c.st == "play" and c.k >= run.n and now > c.t0 + run.n * run.L + 0.3 then
			SetString("savegame.mod.vp.r" .. c.r, string.format("%d,%.1f,%.1f,%d", c.under, c.lateMax * 1000,
				c.lateSum / math.max(1, c.k) * 1000, c.k))
			c.st, c.until_ = "pause", now + PAUSE
		end
	end
	SetInt("savegame.mod.vp.run", c.r or 0)
	SetString("savegame.mod.vp.st", c.st)
	SetInt("savegame.mod.vp.n", c.n)
end

function client.draw()
	local c = client
	local run = VP_RUNS[c.r or 0]
	local line = "VOICE PROBE   "
	if c.st == "done" then
		line = line .. "DONE - how did each run sound? You can quit."
	elseif not run then
		line = line .. "no plan: run probes/voiceprobe/probe.py first"
	elseif c.st == "wait" then
		line = line .. "next: " .. run.id .. " - " .. run.label
	else
		line = line .. "RUN " .. run.id .. " - " .. run.label
	end
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
	if run and run.spatial and c.st == "play" and c.point then          -- (where the voice is)
		local x, y, dist = UiWorldToPixel(c.point)
		if dist > 0 then
			UiPush()
			UiTranslate(x, y)
			UiAlign("center middle")
			UiFont("bold.ttf", 22)
			UiColor(1, 0.9, 0.4)
			UiText("VOICE")
			UiPop()
		end
	end
end
