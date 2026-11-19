// Frida 17 agent: copy plaintext of OpenSSL reads and writes in the MAX desktop client.
// Each chunk goes to the host as send({t: "ssl", dir, ssl, host, fn}, bytes).
// The host decides what to keep; this agent never writes files.
"use strict";

const LIB = "libssl-3-x64.dll";
let hooked = false;

function hookLibssl(mod) {
  if (hooked) {
    return;
  }
  hooked = true;

  const getServername = new NativeFunction(
    mod.getExportByName("SSL_get_servername"), "pointer", ["pointer", "int"]);
  const hostCache = new Map();
  function hostOf(ssl) {
    const key = ssl.toString();
    const known = hostCache.get(key);
    if (known) {
      return known;
    }
    let host = "";
    try {
      const p = getServername(ssl, 0); // TLSEXT_NAMETYPE_host_name
      host = p.isNull() ? "" : p.readCString();
    } catch (e) {
      host = "?";
    }
    if (host) {
      hostCache.set(key, host);
    }
    return host;
  }

  function emit(dir, fn, ssl, buf, n) {
    if (n <= 0) {
      return;
    }
    send({ t: "ssl", dir, fn, ssl: ssl.toString(), host: hostOf(ssl) }, buf.readByteArray(n));
  }

  // int SSL_write(SSL *s, const void *buf, int num) -> bytes written
  Interceptor.attach(mod.getExportByName("SSL_write"), {
    onEnter(args) { this.ssl = args[0]; this.buf = args[1]; },
    onLeave(ret) { emit("out", "SSL_write", this.ssl, this.buf, ret.toInt32()); },
  });
  // int SSL_read(SSL *s, void *buf, int num) -> bytes read
  Interceptor.attach(mod.getExportByName("SSL_read"), {
    onEnter(args) { this.ssl = args[0]; this.buf = args[1]; },
    onLeave(ret) { emit("in", "SSL_read", this.ssl, this.buf, ret.toInt32()); },
  });
  // int SSL_write_ex(SSL *s, const void *buf, size_t num, size_t *written) -> 1 on success
  Interceptor.attach(mod.getExportByName("SSL_write_ex"), {
    onEnter(args) { this.ssl = args[0]; this.buf = args[1]; this.out = args[3]; },
    onLeave(ret) {
      if (ret.toInt32() === 1 && !this.out.isNull()) {
        emit("out", "SSL_write_ex", this.ssl, this.buf, this.out.readU64().toNumber());
      }
    },
  });
  // int SSL_read_ex(SSL *s, void *buf, size_t num, size_t *readbytes) -> 1 on success
  Interceptor.attach(mod.getExportByName("SSL_read_ex"), {
    onEnter(args) { this.ssl = args[0]; this.buf = args[1]; this.out = args[3]; },
    onLeave(ret) {
      if (ret.toInt32() === 1 && !this.out.isNull()) {
        emit("in", "SSL_read_ex", this.ssl, this.buf, this.out.readU64().toNumber());
      }
    },
  });
  send({ t: "hooked", module: mod.path });
}

const already = Process.findModuleByName(LIB);
if (already) {
  hookLibssl(already);
} else {
  Process.attachModuleObserver({
    onAdded(mod) {
      if (mod.name.toLowerCase() === LIB) {
        hookLibssl(mod);
      }
    },
  });
  send({ t: "waiting", module: LIB });
}
