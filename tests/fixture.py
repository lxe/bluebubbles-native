from email.parser import BytesParser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit
import argparse
import json
from io import BytesIO
import queue
import struct
import time
import zlib


parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, default=0)
parser.add_argument("--ready", type=Path)
parser.add_argument("--receipts", type=Path)
parser.add_argument("--video", type=Path)
parser.add_argument("--message-delay", type=float, default=0)
parser.add_argument("--preview-delay", type=float, default=0)
parser.add_argument("--original-delay", type=float, default=0)
parser.add_argument("--query-delay", type=float, default=0)
parser.add_argument("--extra-chats", type=int, default=0)
parser.add_argument("--group-chat", action="store_true")
parser.add_argument("--realtime", action="store_true")
parser.add_argument("--reaction-delay", type=float, default=0)
parser.add_argument("--reject-reaction")
parser.add_argument("--send-delay", type=float, default=0)
parser.add_argument("--reject-text")
parser.add_argument("--large-image", action="store_true")
args = parser.parse_args()
socket_enabled = True
packets = queue.Queue()

FILE_GUID = "fixture-attachment"
IMAGE_GUID = "fixture-image"
VIDEO_GUID = "fixture-video"
FILE_BYTES = b"Attachment fixture\n"
PASSWORD = "fixture-secret"
RECENT_CHAT = "iMessage;-;+15550100100"
OLDER_CHAT = "iMessage;-;+15550100101"
LINK_CHAT = "iMessage;-;+15550100102"
GROUP_CHAT = "iMessage;+;fixture-group"
MAYA = "+15550100100"
JO = "+15550100103"
RECENT = 1795566660000
OLD = 1795500000000

messages = {
    RECENT_CHAT: [
        {"guid": "media-message", "text": "Check the trail markers.\nThe path continues past the bridge.", "dateCreated": RECENT + 1,
         "attachments": [
             {"guid": IMAGE_GUID, "transferName": "Trail markers.png", "mimeType": "image/png"},
             *([{"guid": VIDEO_GUID, "transferName": "Trail video.webm", "mimeType": "video/webm"}] if args.video else []),
         ]},
        {"guid": "photo-message", "text": "Here is the field log.", "dateCreated": RECENT,
         "attachments": [{"guid": FILE_GUID, "transferName": "field-log.txt"}]},
        *[{"guid": f"history-{index}", "text": f"History entry {index}", "dateCreated": OLD + index}
          for index in range(52, 0, -1)],
    ],
    OLDER_CHAT: [{"guid": "older", "text": "See you tomorrow.", "dateCreated": OLD}],
    LINK_CHAT: [
        {"guid": "rich-link", "text": "https://example.invalid/article", "dateCreated": OLD - 1,
         "isFromMe": True, "attachments": [
             {"guid": "apple-link-preview", "transferName": "example.pluginPayloadAttachment"},
         ]},
    ],
}
if args.group_chat:
    earlier = RECENT - 4 * 60 * 60 * 1000
    messages[GROUP_CHAT] = [
        {"guid": "group-link-out", "text": "https://example.invalid/group-preview", "dateCreated": RECENT + 9,
         "isFromMe": True},
        {"guid": "group-out-like", "dateCreated": RECENT + 9, "handle": {"address": MAYA},
         "associatedMessageGuid": "p:0/group-link-out", "associatedMessageType": 2001},
        {"guid": "group-out-laugh", "dateCreated": RECENT + 9, "handle": {"address": JO},
         "associatedMessageGuid": "p:0/group-link-out", "associatedMessageType": 2003},
        {"guid": "group-link-in", "text": "https://example.invalid/group-preview", "dateCreated": RECENT + 8,
         "handle": {"address": MAYA}},
        {"guid": "group-plain", "text": "Water is packed.", "dateCreated": earlier + 8,
         "handle": {"address": JO}, "replyToGuid": "group-reply"},
        {"guid": "group-reply", "text": "I can meet you there.", "dateCreated": earlier + 7,
         "handle": {"address": JO}, "threadOriginatorGuid": "p:0/group-parent", "threadOriginatorPart": "0:0:10"},
        {"guid": "group-like", "dateCreated": earlier + 6, "handle": {"address": MAYA},
         "associatedMessageGuid": "p:0/group-parent", "associatedMessageType": 2001},
        {"guid": "group-tapback", "text": "Loved the message", "dateCreated": earlier + 6,
         "handle": {"address": JO}, "associatedMessageGuid": "p:0/group-parent", "associatedMessageType": 2003},
        {"guid": "group-parent", "text": "Meet by the trailhead.", "dateCreated": earlier + 5,
         "handle": {"address": MAYA}},
    ]
for index in range(args.extra_chats):
    guid = f"iMessage;-;+155502{index:05d}"
    messages[guid] = [{"guid": f"extra-{index}", "text": "Fixture chat", "dateCreated": OLD - 2 - index}]


def sample_png(group=False):
    def block(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    width, height = (320, 200) if group else (200, 320)
    orientation = b"" if group else block(
        b"eXIf", bytes.fromhex("49492a0008000000010012010300010000000600000000000000")
    )
    pixels = b"".join(
        b"\0" + bytes(channel for x in range(width) for channel in
                      ((200 + x // 8, 25 + y // 8, 125) if group
                       else (22 + (height - 1 - y) // 4, 60 + x // 2, 133 + (height - 1 - y) // 4)))
        for y in range(height)
    )
    return (b"\x89PNG\r\n\x1a\n"
            + block(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + orientation + block(b"IDAT", zlib.compress(pixels)) + block(b"IEND", b""))


def large_portrait():
    from PIL import Image
    photo = Image.new("RGB", (3200, 4800), (32, 126, 205))
    photo.paste((210, 93, 38), (0, 0, 1600, 2400))
    photo.paste((79, 170, 111), (1600, 2400, 3200, 4800))
    buffer = BytesIO()
    photo.save(buffer, format="PNG")
    return buffer.getvalue()


original = large_portrait() if args.large_image else None

def receipt(kind, **fields):
    if args.receipts:
        with args.receipts.open("a") as out:
            print(json.dumps({"event": kind, **fields}), file=out)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def respond(self, payload, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({"status": status, "data": payload}).encode())

    def packet(self, payload):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain;charset=UTF-8")
        self.end_headers()
        try:
            self.wfile.write(payload.encode())
        except BrokenPipeError:
            pass

    def route(self):
        url = urlsplit(self.path)
        parameters = parse_qs(url.query)
        if parameters.get("guid") != [PASSWORD]:
            self.respond(None, 401)
            return None, None
        return unquote(url.path).removeprefix("/api/v1"), parameters

    def do_GET(self):
        path, parameters = self.route()
        if not path:
            return
        if path == "/socket.io/" and args.realtime:
            if "sid" not in parameters:
                if socket_enabled:
                    self.packet('0{"sid":"fixture-socket","pingInterval":1500,"pingTimeout":5000}')
                else:
                    self.respond(None, 503)
            else:
                try:
                    self.packet(packets.get(timeout=1.5))
                except queue.Empty:
                    self.packet("2")
        elif path == "/server/info":
            receipt("info")
            self.respond({"server_version": "fixture", "private_api": args.group_chat, "helper_connected": args.group_chat})
        elif path == "/chat/count":
            self.respond({"total": len(messages)})
        elif path == "/contact":
            receipt("contacts", avatar=parameters.get("extraProperties") == ["avatar"])
            self.respond([
                {"displayName": "Maya Chen", "phoneNumbers": [{"address": "+1 555 010 0100"}]},
                {"displayName": "Bike Workshop", "phoneNumbers": [{"address": "+1 555 010 0101"}]},
                {"displayName": "Link Preview", "phoneNumbers": [{"address": "+1 555 010 0102"}]},
                {"displayName": "Jo Reyes", "phoneNumbers": [{"address": JO}]},
            ])
        elif path.startswith("/chat/") and path.endswith("/message"):
            guid = path.split("/")[2]
            offset = int(parameters["offset"][0])
            limit = int(parameters["limit"][0])
            receipt("read", guid=guid, offset=offset)
            time.sleep(args.message_delay)
            self.respond(messages.get(guid, [])[offset:offset + limit])
        elif path == f"/chat/{GROUP_CHAT}/icon" and args.group_chat:
            receipt("group-icon")
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.end_headers()
            self.wfile.write(sample_png(group=True))
        elif path.startswith("/attachment/") and path.endswith("/download"):
            guid = path.split("/")[2]
            files = {
                FILE_GUID: ("text/plain", "field-log.txt", FILE_BYTES),
                IMAGE_GUID: ("image/png", "Trail markers.png", sample_png()),
                "live-fixture-image": ("image/png", "Live trail.png", sample_png()),
            }
            if args.video:
                files[VIDEO_GUID] = ("video/webm", "Trail video.webm", args.video.read_bytes())
            if guid not in files:
                self.respond(None, 404)
                return
            mime, name, contents = files[guid]
            full_size = parameters.get("original") != ["false"]
            receipt("download", guid=guid, original=full_size)
            if original and guid == IMAGE_GUID and full_size:
                contents = original
            if not full_size:
                time.sleep(args.preview_delay)
            else:
                time.sleep(args.original_delay)
            self.send_response(200)
            self.send_header("Content-Type", mime)
            self.send_header("Content-Disposition", f'attachment; filename="{name}"')
            self.end_headers()
            self.wfile.write(contents)
        else:
            self.respond(None, 404)

    def do_POST(self):
        global socket_enabled
        path, _ = self.route()
        if not path:
            return
        body = self.rfile.read(int(self.headers["Content-Length"]))
        if path == "/socket.io/" and args.realtime:
            if body == b"40":
                receipt("socket-connected")
                packets.put('40{"sid":"fixture-namespace"}')
            elif body == b"3":
                receipt("socket-pong")
            self.packet("ok")
        elif path == "/fixture/live" and args.realtime:
            socket_enabled = json.loads(body)["enabled"]
            if not socket_enabled:
                packets.put("1")
            self.respond({})
        elif path == "/fixture/event" and args.realtime:
            item = json.loads(body)
            guid = item["chat"]
            data = dict(item["message"], chats=[{"guid": guid}])
            rows = messages.setdefault(guid, [])
            rows[:] = [message for message in rows if message["guid"] != data["guid"]]
            rows.insert(0, data)
            rows.sort(key=lambda message: message["dateCreated"], reverse=True)
            if item.get("publish", True):
                packets.put("42" + json.dumps([item.get("event", "new-message"), data]))
            self.respond({})
        elif path == "/chat/query":
            incoming = json.loads(body)
            offset, limit = incoming["offset"], incoming["limit"]
            receipt("chats", offset=offset, limit=limit)
            # Deliberately return the older chat first.
            rows = [
                {"guid": guid, "participants": [{"address": person} for person in
                                              ([MAYA, JO] if guid == GROUP_CHAT else [guid.split(";")[-1]])],
                 "lastMessage": messages[guid][-1], "latestMessage": messages[guid][0],
                 **({"style": 43, "properties": [{"groupPhotoGuid": "fixture-group-photo"}]}
                    if guid == GROUP_CHAT else {})}
                for guid in [OLDER_CHAT, RECENT_CHAT, LINK_CHAT, *list(messages)[3:]]
            ]
            time.sleep(args.query_delay)
            self.respond(rows[offset:offset + limit])
        elif path == "/message/text":
            item = json.loads(body)
            receipt("text", guid=item["chatGuid"], text=item["message"], method=item["method"],
                    selected=item.get("selectedMessageGuid"), part=item.get("partIndex"))
            time.sleep(args.send_delay)
            receipt("text-reply", text=item["message"])
            if item["message"] == args.reject_text:
                self.respond({}, 400)
                return
            self.record_message(item["chatGuid"], "sent-" + item["tempGuid"], item["message"],
                                return_message=True, echo=item["tempGuid"],
                                threadOriginatorGuid=item.get("selectedMessageGuid"))
        elif path == "/message/react":
            item = json.loads(body)
            receipt("reaction", guid=item["chatGuid"], target=item["selectedMessageGuid"],
                    reaction=item["reaction"], at=time.time_ns() // 1_000_000)
            time.sleep(args.reaction_delay)
            receipt("reaction-reply", reaction=item["reaction"], at=time.time_ns() // 1_000_000)
            if item["reaction"] == args.reject_reaction:
                self.respond({}, 400)
                return
            chat = item["chatGuid"]
            self.record_message(chat, "reaction-" + str(len(messages[chat])), "", return_message=True,
                                associatedMessageGuid="p:0/" + item["selectedMessageGuid"],
                                associatedMessageType=item["reaction"])
        elif path == "/message/attachment":
            envelope = BytesParser().parsebytes(
                b"Content-Type: " + self.headers["Content-Type"].encode() + b"\r\n\r\n" + body)
            fields = {
                part.get_param("name", header="content-disposition"): part.get_payload(decode=True)
                for part in envelope.get_payload()
            }
            receipt("upload", guid=fields["chatGuid"].decode(), name=fields["name"].decode(),
                    method=fields["method"].decode(), size=len(fields["attachment"]))
            temp_guid = fields["tempGuid"].decode()
            self.record_message(fields["chatGuid"].decode(), "sent-" + temp_guid,
                                "Attachment received", return_message=True, echo=temp_guid)
        else:
            self.respond(None, 404)

    def record_message(self, chat, guid, text, return_message=False, echo=None, **fields):
        if chat not in messages:
            self.respond(None, 404)
            return
        timestamp = max(int(time.time() * 1000), messages[chat][0]["dateCreated"] + 1)
        message = {"guid": guid, "text": text, "dateCreated": timestamp, "isFromMe": True, **fields}
        messages[chat].insert(0, message)
        if echo and args.realtime:
            packets.put("42" + json.dumps(["new-message", dict(message, chats=[{"guid": chat}], tempGuid=echo)]))
        self.respond((dict(message, tempGuid=echo) if echo else message) if return_message else {})


with ThreadingHTTPServer(("127.0.0.1", args.port), Handler) as server:
    if args.ready:
        args.ready.write_text(str(server.server_port))
    else:
        print(server.server_port, flush=True)
    server.serve_forever()
