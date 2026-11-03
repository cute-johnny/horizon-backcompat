import javax.crypto.Cipher;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.math.BigInteger;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.MessageDigest;
import java.util.Arrays;

/**
 * A 1.8 (protocol 47) online-mode server that goes as far as the encryption response, for the
 * bridge's end-to-end test: java MockLegacyServer.java
 * <p>
 * Prints "PORT n" once listening, then for the first login "NAME name" and, after decrypting the
 * shared secret, "HASH h": the session hash a real server would check with hasJoined, which the
 * client must have joined with. Exits after one login.
 */
public class MockLegacyServer {

    public static void main(String[] args) throws Exception {
        final KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
        generator.initialize(1024);
        final KeyPair keys = generator.generateKeyPair();
        try (ServerSocket server = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            System.out.println("PORT " + server.getLocalPort());
            System.out.flush();
            while (true) {
                try (Socket socket = server.accept()) {
                    socket.setSoTimeout(30_000);
                    if (serve(socket, keys)) {
                        return;
                    }
                }
            }
        }
    }

    /** True once a login reached the encryption response. */
    private static boolean serve(Socket socket, KeyPair keys) throws Exception {
        final DataInputStream in = new DataInputStream(socket.getInputStream());
        final OutputStream out = socket.getOutputStream();
        final byte[] handshake = readFrame(in);
        final Reader h = new Reader(handshake);
        h.varInt(); // id 0
        final int protocol = h.varInt();
        h.string();
        h.u16();
        final int next = h.varInt();
        if (next == 1) {
            // Status: answer as 1.8 would, then the ping.
            readFrame(in);
            writeFrame(out, packet(0, string("{\"version\":{\"name\":\"1.8.9\",\"protocol\":47},\"players\":{\"max\":1,\"online\":0},\"description\":{\"text\":\"mock\"}}")));
            final byte[] ping = readFrame(in);
            writeFrame(out, ping);
            return false;
        }
        System.out.println("PROTOCOL " + protocol);
        final Reader hello = new Reader(readFrame(in));
        hello.varInt();
        System.out.println("NAME " + hello.string());
        final byte[] token = {1, 2, 3, 4};
        final ByteArrayOutputStream request = new ByteArrayOutputStream();
        request.write(string(""));
        request.write(bytes(keys.getPublic().getEncoded()));
        request.write(bytes(token));
        writeFrame(out, packet(1, request.toByteArray()));
        final Reader response = new Reader(readFrame(in));
        if (response.varInt() != 1) {
            System.out.println("ERROR not an encryption response");
            return true;
        }
        final Cipher rsa = Cipher.getInstance("RSA/ECB/PKCS1Padding");
        rsa.init(Cipher.DECRYPT_MODE, keys.getPrivate());
        final byte[] secret = rsa.doFinal(response.bytes());
        rsa.init(Cipher.DECRYPT_MODE, keys.getPrivate());
        final byte[] echoed = rsa.doFinal(response.bytes());
        if (!Arrays.equals(token, echoed)) {
            System.out.println("ERROR verify token");
            return true;
        }
        final MessageDigest sha1 = MessageDigest.getInstance("SHA-1");
        sha1.update("".getBytes(StandardCharsets.ISO_8859_1));
        sha1.update(secret);
        sha1.update(keys.getPublic().getEncoded());
        System.out.println("HASH " + new BigInteger(sha1.digest()).toString(16));
        System.out.flush();
        return true;
    }

    private static byte[] readFrame(DataInputStream in) throws IOException {
        final int length = readVarInt(in);
        final byte[] frame = new byte[length];
        in.readFully(frame);
        return frame;
    }

    private static void writeFrame(OutputStream out, byte[] body) throws IOException {
        out.write(varInt(body.length));
        out.write(body);
        out.flush();
    }

    private static int readVarInt(InputStream in) throws IOException {
        int value = 0;
        for (int i = 0; i < 5; i++) {
            final int b = in.read();
            if (b < 0) throw new EOFException();
            value |= (b & 0x7F) << (7 * i);
            if ((b & 0x80) == 0) return value;
        }
        throw new IOException("VarInt too big");
    }

    private static byte[] varInt(int value) {
        final ByteArrayOutputStream out = new ByteArrayOutputStream();
        while ((value & ~0x7F) != 0) {
            out.write((value & 0x7F) | 0x80);
            value >>>= 7;
        }
        out.write(value);
        return out.toByteArray();
    }

    private static byte[] string(String s) {
        return bytes(s.getBytes(StandardCharsets.UTF_8));
    }

    private static byte[] bytes(byte[] b) {
        final ByteArrayOutputStream out = new ByteArrayOutputStream();
        out.writeBytes(varInt(b.length));
        out.writeBytes(b);
        return out.toByteArray();
    }

    private static byte[] packet(int id, byte[] payload) {
        final ByteArrayOutputStream out = new ByteArrayOutputStream();
        out.writeBytes(varInt(id));
        out.writeBytes(payload);
        return out.toByteArray();
    }

    private static final class Reader {
        private final byte[] data;
        private int pos;

        Reader(byte[] data) {
            this.data = data;
        }

        int varInt() {
            int value = 0;
            for (int i = 0; i < 5; i++) {
                final int b = this.data[this.pos++] & 0xFF;
                value |= (b & 0x7F) << (7 * i);
                if ((b & 0x80) == 0) return value;
            }
            throw new IllegalStateException("VarInt too big");
        }

        String string() {
            return new String(bytes(), StandardCharsets.UTF_8);
        }

        byte[] bytes() {
            final int length = varInt();
            final byte[] b = Arrays.copyOfRange(this.data, this.pos, this.pos + length);
            this.pos += length;
            return b;
        }

        int u16() {
            final int v = ((this.data[this.pos] & 0xFF) << 8) | (this.data[this.pos + 1] & 0xFF);
            this.pos += 2;
            return v;
        }
    }

}
