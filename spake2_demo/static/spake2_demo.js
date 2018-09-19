
// URLs for linkserver.
const LINKSERVER_CONNECT_URL = 'ws://localhost:8080/ws/';
const LINKSERVER_PUT_URL = 'http://localhost:8080/v1/send/';

// Some basic byte-handling functions.

function randomBytes(size) {
  let bytes = new Uint8Array(size)
  crypto.getRandomValues(bytes)
  return bytes
}

function bytesToHex(bytes) {
  output = []
  for (let i = 0; i < bytes.length; i++) {
    let c = bytes[i].toString(16);
    if (c.length < 2) {
      c = "0" + c
    }
    output.push(c)
  }
  return output.join('')
}

function bytesFromHex(hexstr) {
  if (hexstr.length % 2 !== 0) { throw new Error("odd-length hex string"); }
  let bytes = new Uint8Array(hexstr.length / 2)
  for (let i = 0; i < bytes.length; i ++) {
    bytes[i] = parseInt(hexstr.substr(2 * i, 2), 16)
  }
  return bytes
}


// Function to turn a byte string into a v4 UUID.
// We cheat a little bit here and allow encoding
// short random strings like "1234" as UUIDs with a bunch
// of zero bits, like "00000000-0000-0000-0000-000000001234".
// This lets us create temporary channels on the linkserver,
// which expects UUIDs.

function bytesToUUID(bytes) {
  if (bytes.length > 16) {
    throw new Error("too many bytes for a UUID")
  }
  if (bytes.length < 16) {
    origBytes = bytes
    bytes = new Uint8Array(16)
    let offset = 16 - origBytes.length
    for (let i = 0; i < origBytes.length; i++) {
      bytes[offset + i] = origBytes[i];
    }
  }
  bytes[6] = 0x40 | (bytes[6] & 0x0F)
  bytes[8] = 0x80 | (bytes[8] & 0x3F)
  output = []
  for (let i = 0; i < 16; i++) {
    let c = bytes[i].toString(16);
    if (c.length < 2) {
      c = "0" + c
    }
    output.push(c)
    if (i == 3 || i == 5 || i == 7 || i == 9) {
      output.push("-")
    }
  }
  return output.join('')
}


// A websocket through which we can receive messages.
// This connects to the "linkserver" server found elsewhere
// in this repo, and allows other clients to relay messages
// to this pipe knowing only the identifying UUID.

function Pipe(id, callback) {
  this.id = id
  this.callback = callback
  this.socket = new WebSocket(LINKSERVER_CONNECT_URL);
  this.connected = false
  this.socket.addEventListener('open', (event) => {
    this.socket.send(this.id);
  });
  this.socket.addEventListener('message', (event) => {
    if (!this.connected) {
      if (event.data !== this.id) {
        console.error('Pipe setup error', event);
        this.close();
      }
      this.connected = true
    } else {
      console.log("RECV", this.id, event.data)
      if (this.callback) {
        this.callback(event.data);
      }
    }
  });
};

Pipe.prototype.close = function close() {
  this.socket.close();
  if (this.callback) {
    this.callback(null);
  }
}

// Given a UUID, send a message to the listening Pipe with that UUID.
// Returns a promise, which will reject if there was no receiver.

Pipe.send = function send(id, msg) {
  console.log("SEND", id, msg)
  return fetch(LINKSERVER_PUT_URL + id, {
    method: 'PUT',
    body: msg
  }).then(r => {
    if (r.status !== 200) {
      throw new Error('Failed to send to ' + id + ': ' + r.text());
    }
  })
}

// An abstraction for two-way communication between two Pipe's,
// encrypted with a session key.

function PairedChannel(key, myPipe, theirPipeId) {
  this.keyPromise = crypto.subtle.importKey('raw', bytesFromHex(key), { name: 'AES-GCM' }, false, ['encrypt', 'decrypt'])
  this.myPipe = myPipe
  this.theirPipeId = theirPipeId
  this.onReceive = null
  this.myPipe.callback = (msg) => {
    if (this.onReceive) {
      if (! msg) {
        this.onReceive(null)
      } else {
        this.decrypt(msg).then(dec => {
          this.onReceive(dec)
        })
      }
    }
  }
}

PairedChannel.prototype.close = function() {
  this.myPipe.close()
}

PairedChannel.prototype.send = function(msg) {
  return this.encrypt(msg).then(enc => {
    return Pipe.send(this.theirPipeId, enc)
  })
}

PairedChannel.prototype.encrypt = function(msg) {
  // Quick-n-dirty encrytion by directly using WebCrypto APIs.
  // Don't even think about doing it like this in production!
  return this.keyPromise.then(key => {
    let iv = crypto.getRandomValues(new Uint8Array(12))
    let plaintext = new TextEncoder("utf8").encode(msg)
    return crypto.subtle.encrypt({
      name: 'AES-GCM',
      iv: iv
    }, key, plaintext).then(ciphertext => {
      return JSON.stringify({
        ciphertext: bytesToHex(new Uint8Array(ciphertext)),
        iv: bytesToHex(iv),
      })
    })
  })
}

PairedChannel.prototype.decrypt = function(msg) {
  // Quick-n-dirty decryption by directly using WebCrypto APIs.
  // Don't even think about doing it like this in production!
  return this.keyPromise.then(key => {
    let { ciphertext, iv } = JSON.parse(msg)
    return crypto.subtle.decrypt({
      name: 'AES-GCM',
      iv: bytesFromHex(iv)
    }, key, bytesFromHex(ciphertext)).then(plaintext => {
      return new TextDecoder("utf8").decode(new Uint8Array(plaintext))
    })
  })
}


// OK, now we can actually do the pairing!
// It works as follows:
//
//  * Pairing codes are randomly generated hex strings
//    of the form <channel>-<secret>.
//
//  * The "channel" is used as a temporary device id with
//    the linkserver to allow the initial message exchange,
//    with the socket torn down after the exchange.
//
//  * The consumer generates the pairing code and then:
//      * Listens on the pipe for <channel> for a message
//        from the provider, which will contain the SPAKE2
//        message and the provider's Pipe id.
//      * Completes the SPAKE2 dance using the recieved message,
//        generating the session key and a reply message to send
//        to the provider.
//      * Creates a new Pipe with the linkserver over which
//        to receive encrypted communications.
//      * Sends the reply message and its Pipe id to the provider.
//      * Creates an encrypted two-way channel from the session key,
//        its pipe, and the provider's pipe id.
//
//  * The provider receives the pairing code via user input and then:
//      * Initializes an SPAKE2 round, generatig a message to be send
//        to the consumer.
//      * Creates a new Pipe with the linkserver over which it will
//        receive the consumer's reply as well as subsequent encrypted
//        messages.
//      * Extracts the channel from the pairing code, uses it as a
//        linkserver pipe id, and sends the SPAKE2 message and its
//        own pipe id over this pipe to the consumer.
//      * Receives the reply message from the consumer, containing
//        the SPAKE2 reply and the consumer's pipe id.
//      * Completes the SPAKE2 round using the consumer's reply,
//        to obtain the session key.
//      * Creates an encrypted two-way channel from the session key,
//        its pipe, and the consumers's pipe id.
//
// With short codes, there's obviously a chance that the initial
// handshake will conflict with a channel that's being used by some
// other connection, but this should at least fail securely thanks
// to SPAKE2.

function generatePairingCode() {
  return bytesToHex(randomBytes(2)) + "-" + bytesToHex(randomBytes(2))
}

function pairAsConsumer(code) {
  return Rust.spake2_demo.then(spake2_demo => {
    return new Promise((resolve, reject) => {
      let channel = bytesFromHex(code.split("-")[0])
      let pakePipe = new Pipe(bytesToUUID(channel), resp => {
        Promise.resolve().then(() => {
          if (! resp) { throw new Error('pairing request failed') }
          pakePipe.callback = null
          pakePipe.close()
          const { id, msg } = JSON.parse(resp)
          let r = spake2_demo.oneshot(code, pakePipe.id, msg)
          let myPipe = new Pipe(bytesToUUID(randomBytes(16)))
          return Pipe.send(id, JSON.stringify({
            id: myPipe.id,
            msg: r.msg
          })).then(() => {
            return new PairedChannel(r.key, myPipe, id)
          }).catch(err => {
            myPipe.close()
            throw err
          })
        }).then(resolve, reject)
      })
    })
  })
}

function pairAsProvider(code) {
  return Rust.spake2_demo.then(spake2_demo => {
    return new Promise((resolve, reject) => {
      let channel = bytesFromHex(code.split("-")[0])
      let pakeId = bytesToUUID(channel)
      let msg1 = spake2_demo.start(code, pakeId)
      let myPipe = new Pipe(bytesToUUID(randomBytes(16)), resp => {
        Promise.resolve().then(() => {
          if (!resp) { throw new Error("pairing setup failed") }
          myPipe.callback = null
          const { id, msg } = JSON.parse(resp)
          let key = spake2_demo.finish(code, msg)
          return resolve(new PairedChannel(key, myPipe, id))
        }).then(resolve, reject)
      })
      Pipe.send(pakeId, JSON.stringify({
        id: myPipe.id,
        msg: msg1
      })).catch(err => {
        myPipe.close()
        reject(err);
      })
    })
  })
}
