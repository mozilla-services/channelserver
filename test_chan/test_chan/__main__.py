import time
import os
import base64
import subprocess
import json
import uuid

import psutil
import websocket

from websocket._exceptions import WebSocketConnectionClosedException

proc = None


class Connection(object):
    ws = None
    link = None

    def __init__(self, url=None):
        self.ws = websocket.WebSocket()
        if url:
            self.connect(url)

    def connect(self, url):
        try:
            self.ws.connect(url)
        except ValueError as ex:
            print(ex)
            raise
        body = json.loads(self.ws.recv())
        self.link = body["link"]
        self.channelid = self.link.rsplit('/')[-1]

    def send(self, message):
        self.ws.send(message)

    def recv(self):
        return self.ws.recv()

    def is_closed(self):
        return not self.ws.connected

    def close(self):
        self.ws.close(reason="Goodbye")


class Config(object):
    def __init__(self):
        """Read configuration options as "TEST_" prefix, to prevent
        collisions"""
        # defaults
        opts = {
            "port": "8000",
            "protocol": "ws",
            "app_path": "../target/debug/channelserver",
            "mmdb_loc": "../channelserver/mmdb/latest/GeoLite2-City.mmdb",
            "host": 'localhost',
            "max_exchanges": "10",
            "max_data": "3096",
        }

        # extract options from environment
        for (env, val) in os.environ.items():
            if not env.startswith("TEST_"):
                continue
            opt = env.replace("TEST_", "").lower()
            if opt in opts:
                opts[opt] = val

        # make the options your dict
        self.__dict__ = opts

    def as_env(self):
        """Convert options into ENV vars """
        envs = {}
        for env in vars(self):
            if env == "app_path":
                continue
            envs["PAIR_{}".format(env.upper())] = self.__dict__[env]
        return envs

    def base(self):
        """Return base path to app"""
        return "{}://{}:{}".format(self.protocol, self.host, self.port)


    def base(self):
        return "{}://{}:{}".format(self.protocol, self.host, self.port)


def setup(opts, **kwargs):
    global proc
    envs = opts.as_env()
    for arg in kwargs:
        envs["PAIR_{}".format(arg.upper())] = kwargs[arg]
    cmd = opts.app_path
    print("Starting {} {}".format(" ".join(envs), cmd))
    proc = subprocess.Popen(cmd, shell=True, env=envs)
    time.sleep(0.25)


def shutdown():
    global proc
    try:
        parent = psutil.Process(pid=proc.pid)
        for child in parent.children(recursive=True):
            os.kill(child.pid, 9)
        os.kill(proc.pid, 9)
        time.sleep(2)
        os.wait()
        print("shutdown")
    except Exception as ex:
        print("Kill failed {}", ex)


def get_connection(opts):
    alice = Connection(url=opts.base() + "/v1/ws/")
    bob = Connection(opts.base() + alice.link)
    return alice, bob


def simple_connection(opts):
    print("#### Simple Connection")
    (alice, bob) = get_connection(opts)
    message = "Test message"
    alice.send(message)
    body = json.loads(bob.recv())
    assert message == body["message"]
    assert "sender" in body
    print("===== ok")


def full_exchange(opts):
    print("#### Full Exchange")
    (alice, bob) = get_connection(opts)
    message = """intro message"""
    alice.send(message)
    assert message == json.loads(bob.recv())["message"], "Intro didn't match"
    # channelserv only deals with text currently
    message = base64.b85encode(os.urandom(1024)).decode("utf8")
    alice.send(message)
    reply = json.loads(bob.recv())
    assert message == reply["message"], "Message didn't match"
    print("===== ok")


def max_data(opts, max_bytes=3096):
    print("#### Max Data")
    (alice, bob) = get_connection(opts)
    message = base64.b85encode(os.urandom(max_bytes)).decode("utf8")
    alice.send(message)
    time.sleep(0.5)
    # Call recv to hand the close packet.
    try:
        bob.recv()
    except WebSocketConnectionClosedException:
        pass
    assert bob.is_closed(), "Receiver did not close"
    print("===== ok")


def max_period(opts):
    print("#### Max Period")
    (alice, bob) = get_connection(opts)
    i = 0
    while True:
        message = "This is message #{}".format(i)
        alice.send(message)
        try:
            reply = bob.recv()
        except WebSocketConnectionClosedException:
            break
        if len(reply) == 0:
            break
        reply = json.loads(reply)
        assert message == reply.get("message"), "Message #{} failed".format(i)
        time.sleep(2)
        i += 1
        assert i < 15, "Connection open too long?"

    assert bob.is_closed(), "Receiver did not close"
    print("===== ok")


def max_exchange(opts, max_exchange=10):
    print("#### Max Exchange")
    (alice, bob) = get_connection(opts)
    for i in range(0, max_exchange+1):
        message = "This is message #{}".format(i)
        alice.send(message)
        try:
            reply = bob.recv()
        except WebSocketConnectionClosedException:
            assert i == max_exchange, "Invalid message count {}".format(i)
            break
        if len(reply) == 0:
            break
        reply = json.loads(reply)
        assert message == reply.get("message"), "Message #{} failed".format(i)
    try:
        # Try reading, ignoring an already closed socket.
        bob.recv()
    except WebSocketConnectionClosedException:
        pass

    assert bob.is_closed(), "Receiver did not close"
    print("===== ok")


def bad_connection(opts):
    print("#### Bad connection")
    alice = Connection(url=opts.base() + "/v1/ws/")
    bad_link = alice.link.rsplit('/', 1)[0]
    bob_chan = uuid.uuid4().hex
    assert bob_chan != alice.channelid, "Oops, matching channelids"
    try:
        bob = Connection(url="{}{}/{}".format(opts.base(), bad_link,
                            bob_chan))
        # Try reading, ignoring an already closed socket.
        bob.recv()
        assert bob.is_closed(), "Receiver did not close."
    except websocket._exceptions.WebSocketConnectionClosedException:
        pass
    print("===== ok")


def main():
    opts = Config()
    try:
        setup(opts, max_data="2048")
        simple_connection(opts)
        full_exchange(opts)
        max_data(opts, 3096)
        max_exchange(opts)
        max_period(opts)
        bad_connection(opts)
        print("\n\n All tests passed")
    except Exception as ex:
        print("ERR:: {}".format(ex))
        raise
    finally:
        shutdown()


if __name__ == "__main__":
    main()
