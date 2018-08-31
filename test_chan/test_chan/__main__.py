import time
import os
import base64
import subprocess
import json

import psutil
import websocket
import websocket._abnf as abnf

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

    def send(self, message):
        self.ws.send(message)

    def recv(self):
        return self.ws.recv()

    def is_closed(self):
        return not self.ws.connected

    def close(self):
        self.ws.close(reason="Goodbye")


class Config(object):
    protocol = "ws"
    host = "localhost"
    port = "8000"
    app_path = "../target/debug/channelserver"
    max_data = "0"

    def base(self):
        return "{}://{}:{}".format(self.protocol, self.host, self.port)


def setup(opts, **kwargs):
    global proc
    envs = {}
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
    (alice, bob) = get_connection(opts)
    message = "Test message"
    alice.send(message)
    body = json.loads(bob.recv())
    assert message == body["message"]
    assert "sender" in body
    print("ok")


def full_exchange(opts):
    (alice, bob) = get_connection(opts)
    message = """intro message"""
    alice.send(message)
    assert message == json.loads(bob.recv())["message"], "Intro didn't match"
    # channelserv only deals with text currently
    message = base64.b85encode(os.urandom(2000)).decode("utf8")
    alice.send(message)
    bm = json.loads(bob.recv())
    assert message == bm["message"], "Message didn't match"
    print("ok")


def max_data(opts, max_bytes=2048):
    (alice, bob) = get_connection(opts)
    message = base64.b85encode(os.urandom(max_bytes)).decode("utf8")
    alice.send(message)
    assert bob.is_closed(), "Receiver did not close"


def max_exchange(opts, max_exchange=5):
    (alice, bob) = get_connection(opts)
    for i in range(0, max_exchange):
        message = "This is message #{}".format(i)
        alice.send(message)
        assert message == bob.recv(), "Message #{} failed".format(i)
    assert bob.is_closed(), "Receiver did not close"


def main():
    opts = Config()
    try:
        setup(opts)
        simple_connection(opts)
        full_exchange(opts)
    except Exception as ex:
        print("ERR:: {}".format(ex))
        raise
    finally:
        shutdown()
    try:
        setup(opts, max_data="200")
        max_data(opts, 400)
        max_exchange(opts, 5)
        print("\n\n All tests passed")
    except Exception as ex:
        print("ERR::", ex)
        raise
    finally:
        shutdown()


if __name__ == "__main__":
    main()
