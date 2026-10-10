## Koetama for a Godot 4 game (PROTOCOL.md, the socket transport): add this node (an autoload named Koetama is
## easiest), set the feed's fields as the game changes, and connect to the signals.
##
##     Koetama.port = 47150                       # (the game mod's profile: {"type": "socket", "port": 47150})
##     Koetama.feed.listen = "push_to_talk"       # every field of PROTOCOL.md "Game -> Koetama: the feed"
##     Koetama.feed.talk_key = Input.is_action_pressed("talk")
##     Koetama.speech.connect(func(kind, utt, text, obj): if kind == "final": chat.say(text))
##     Koetama.talking.connect(func(id, on): heads[id].speaking_icon.visible = on)
##
## Voices: give the feed this player's id ("me": a number or a string, a Steam id), a room ("room_seed"), where they
## hear from ("listener": position, forward, right, up - Godot's camera looks down -z, so give all three) and how far
## their voice reaches ("range": [near, far]), and each other player's "position" in "speakers": Koetama works out
## loudness, direction and who gets the voice. The game adds only "muffle" (its raycast: walls).
##
## Translation: put the chat lines the game shows in "to_translate" ([{"id", "text"}]) and show each `translation`
## that comes back. What they are translated into is the player's own setting in Koetama's window ("Translate chat
## into"), told back in `translations_status` (`translate_into`); "translate": false in the feed stops it for a while.
##
## The node connects to 127.0.0.1 (and again whenever Koetama restarts), sends the feed when it changes and at
## least once a second, and turns each line Koetama sends into a signal. Godot 3 / Webfishing: the same with
## StreamPeerTCP's 3.x names (connect_to_host, get_status, get_partial_data) and JSON.parse(...).result.
extends Node

## the profile's port
@export var port: int = 47150

## Koetama's hello: what it does for this game ("speech", "voices", "rooms", "translate")
signal hello(features: Array)
## what the player said: kind "start" (no text yet), "live" (the words so far), "final" (the line)
signal speech(kind: String, utt: int, text: String, obj: Dictionary)
## a new voice room for the session (the host keeps the first and shares it with every player)
signal room(room: String, key: String)
## the voice chat: state "off", "connecting", "connected", "unreachable" or "id_taken" (another player's id clashes
## with this one in this room: no voice this session); players: the other players' ids whose Koetama is in the room
signal voice(state: String, players: Array)
## a player's voice started (true) or stopped (false) being heard here - or this player's own (id = feed.me) being
## sent: for speaking icons. Ids are Strings here (a number id as its digits)
signal talking(id: String, talking: bool)
## speech to text: "off", "loading", "ready", "error"; the microphone: "closed", "open", "none". Lines said before
## "ready" and "open" are not heard
signal status(speech: String, microphone: String)
## the translation of line `id` ("": nothing to show), and the translation used (from -> to; "" when not said)
signal translation(id: int, text: String, from: String, to: String)
## what chat is translated into (the player's setting in Koetama's window; "": off), and the state of each pair in
## use: [{from, to, state, progress?}]
signal translations_status(into: String, translations: Array)

## the feed (PROTOCOL.md): change any field; it is sent when it changes
var feed: Dictionary = {"type": "feed", "listen": "off", "lang": "en", "live": true, "speakers": []}
## what the hello said (empty: not connected yet)
var features: Array = []
## the players whose voice is heard now (and this player's own while it is sent): id -> true
var talking_now: Dictionary = {}
## the last status: {"speech": .., "microphone": ..} (empty: none yet)
var last_status: Dictionary = {}
## what Koetama translates chat into, from the last translations_status ("": off, or not said yet)
var translate_into: String = ""

var _tcp := StreamPeerTCP.new()
var _buf := PackedByteArray()
var _sent := ""
var _sent_at := -1e9
var _retry_at := 0.0


func connected() -> bool:
	return _tcp.get_status() == StreamPeerTCP.STATUS_CONNECTED


func _process(_delta: float) -> void:
	var now := Time.get_ticks_msec() / 1000.0
	_tcp.poll()
	match _tcp.get_status():
		StreamPeerTCP.STATUS_NONE, StreamPeerTCP.STATUS_ERROR:
			features = []
			talking_now.clear()
			last_status = {}
			translate_into = ""
			if now >= _retry_at:                         # (Koetama may start after the game: try every 2 s)
				_retry_at = now + 2.0
				_tcp = StreamPeerTCP.new()
				_tcp.connect_to_host("127.0.0.1", port)
				_sent = ""
		StreamPeerTCP.STATUS_CONNECTED:
			_read()
			var line := JSON.stringify(feed)
			if line != _sent or now - _sent_at >= 0.5:  # (on a change, and twice a second: Koetama's 1.5 s timeout)
				_tcp.put_data((line + "\n").to_utf8_buffer())
				_sent = line
				_sent_at = now


func _read() -> void:
	var avail := _tcp.get_available_bytes()
	if avail <= 0:
		return
	var got: Array = _tcp.get_partial_data(avail)
	if got[0] != OK:
		return
	_buf.append_array(got[1])
	while true:
		var nl := _buf.find(10)                          # ("\n" ends each object)
		if nl < 0:
			break
		var text := _buf.slice(0, nl).get_string_from_utf8()
		_buf = _buf.slice(nl + 1)
		var obj = JSON.parse_string(text)
		if typeof(obj) == TYPE_DICTIONARY:
			_dispatch(obj)


func _dispatch(o: Dictionary) -> void:
	match o.get("type", ""):
		"hello":
			features = o.get("features", [])
			hello.emit(features)
		"speech":
			speech.emit(o.get("kind", ""), int(o.get("utt", 0)), o.get("text", ""), o)
		"room":
			room.emit(o.get("room", ""), o.get("key", ""))
		"voice":
			var players: Array = []
			for p in o.get("players", []):
				players.append(_id(p))
			voice.emit(o.get("state", ""), players)
		"talking":
			var id := _id(o.get("id", ""))
			var on := bool(o.get("talking", false))
			if on:
				talking_now[id] = true
			else:
				talking_now.erase(id)
			talking.emit(id, on)
		"status":
			last_status = {"speech": o.get("speech", ""), "microphone": o.get("microphone", "")}
			status.emit(last_status.speech, last_status.microphone)
		"translation":
			translation.emit(int(o.get("id", 0)), o.get("text", ""), o.get("from", ""), o.get("to", ""))
		"translations_status":
			translate_into = str(o.get("into", ""))
			translations_status.emit(translate_into, o.get("translations", []))
		# (another type: a newer Koetama's - ignored)


## A player id as a String: JSON numbers arrive as floats in Godot (12 -> "12").
func _id(v) -> String:
	if typeof(v) == TYPE_FLOAT or typeof(v) == TYPE_INT:
		return str(int(v))
	return str(v)
