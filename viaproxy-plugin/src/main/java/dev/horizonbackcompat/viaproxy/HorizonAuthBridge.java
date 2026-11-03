package dev.horizonbackcompat.viaproxy;

import io.netty.channel.Channel;
import io.netty.channel.ChannelFutureListener;
import net.lenni0451.lambdaevents.EventHandler;
import net.raphimc.netminecraft.constants.ConnectionState;
import net.raphimc.netminecraft.packet.Packet;
import net.raphimc.netminecraft.packet.impl.login.C2SLoginCustomQueryAnswerPacket;
import net.raphimc.netminecraft.packet.impl.login.S2CLoginCustomQueryPacket;
import net.raphimc.viaproxy.ViaProxy;
import net.raphimc.viaproxy.plugins.ViaProxyPlugin;
import net.raphimc.viaproxy.plugins.events.ConnectEvent;
import net.raphimc.viaproxy.plugins.events.JoinServerRequestEvent;
import net.raphimc.viaproxy.proxy.packethandler.PacketHandler;
import net.raphimc.viaproxy.proxy.session.ProxyConnection;
import net.raphimc.viaproxy.util.logging.Logger;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Lets ViaProxy join online-mode servers with the account of the Horizon client connected to it.
 * <p>
 * ViaProxy runs with no account (auth-method NONE). When a server asks for a session join,
 * this plugin sends the client a login query on {@link #CHANNEL} with the server's hash, waits for its
 * answer (1: joined; 0 and a UTF-8 reason: failed) and lets the login go on. The client's access token
 * never reaches ViaProxy. Horizon's backward compatibility plugin answers this query only on the
 * connections it opened to its own local ViaProxy.
 */
public class HorizonAuthBridge extends ViaProxyPlugin {

    public static final String CHANNEL = "horizon_backcompat:join";
    private static final long TIMEOUT_SECONDS = 20;

    /** The process id of the Horizon that started this ViaProxy: it stops when that process is gone. */
    public static final String PARENT_PROPERTY = "horizon.backcompat.parent";

    @Override
    public void onEnable() {
        ViaProxy.EVENT_MANAGER.register(this);
        watchParent();
    }

    private static void watchParent() {
        final String parent = System.getProperty(PARENT_PROPERTY);
        if (parent == null) {
            return;
        }
        final ProcessHandle handle;
        try {
            handle = ProcessHandle.of(Long.parseLong(parent)).orElse(null);
        } catch (NumberFormatException e) {
            return;
        }
        if (handle == null) {
            System.exit(0);
        }
        final Thread watchdog = new Thread(() -> {
            handle.onExit().join();
            Logger.LOGGER.info("Horizon (process " + parent + ") exited: stopping ViaProxy");
            System.exit(0);
        }, "Horizon parent watchdog");
        watchdog.setDaemon(true);
        watchdog.start();
    }

    @EventHandler
    public void onConnect(final ConnectEvent event) {
        // First, so the answers never reach the server.
        event.getProxyConnection().getPacketHandlers().add(0, new AnswerHandler(event.getProxyConnection()));
    }

    @EventHandler
    public void onJoinServerRequest(final JoinServerRequestEvent event) {
        final ProxyConnection connection = event.getProxyConnection();
        final AnswerHandler answers = connection.getPacketHandler(AnswerHandler.class);
        final Channel client = connection.getC2P();
        if (answers == null || connection.getC2pConnectionState() != ConnectionState.LOGIN) {
            return;
        }
        if (client.eventLoop().inEventLoop()) {
            // The answer arrives on this thread: waiting for it here would never end.
            Logger.u_err("horizon-auth", connection, "Cannot ask the client to join from its own event loop");
            return;
        }
        final int queryId = answers.nextQueryId();
        final CompletableFuture<byte[]> answer = answers.expect(queryId);
        final byte[] hash = event.getServerIdHash().getBytes(StandardCharsets.UTF_8);
        client.writeAndFlush(new S2CLoginCustomQueryPacket(queryId, CHANNEL, hash)).addListener(ChannelFutureListener.FIRE_EXCEPTION_ON_FAILURE);
        try {
            final byte[] response = answer.get(TIMEOUT_SECONDS, TimeUnit.SECONDS);
            if (response != null && response.length >= 1 && response[0] == 1) {
                Logger.u_info("horizon-auth", connection, "Joined the online mode server with the client's account");
                event.setCancelled(true);
            } else if (response == null || response.length == 0) {
                Logger.u_err("horizon-auth", connection, "The client does not answer session join requests (not Horizon with backward compatibility?)");
            } else {
                final String reason = new String(Arrays.copyOfRange(response, 1, response.length), StandardCharsets.UTF_8);
                Logger.u_err("horizon-auth", connection, "The client could not join: " + reason);
            }
        } catch (Exception e) {
            Logger.u_err("horizon-auth", connection, "No session join answer from the client: " + e);
        } finally {
            answers.forget(queryId);
        }
        // Not cancelled: ViaProxy disconnects with its "requires a valid authentication mode" message.
    }

    /** Takes the client's answers to this plugin's queries out of the stream to the server. */
    public static final class AnswerHandler extends PacketHandler {

        /** "HB" and a counter: far from the small ids servers and proxies use for their own queries. */
        private static final AtomicInteger NEXT_ID = new AtomicInteger(0x48420000);

        private final Map<Integer, CompletableFuture<byte[]>> pending = new ConcurrentHashMap<>();

        public AnswerHandler(final ProxyConnection proxyConnection) {
            super(proxyConnection);
        }

        int nextQueryId() {
            return NEXT_ID.getAndIncrement();
        }

        CompletableFuture<byte[]> expect(final int queryId) {
            final CompletableFuture<byte[]> future = new CompletableFuture<>();
            this.pending.put(queryId, future);
            return future;
        }

        void forget(final int queryId) {
            this.pending.remove(queryId);
        }

        @Override
        public boolean handleC2P(final Packet packet, final List<ChannelFutureListener> listeners) {
            if (packet instanceof C2SLoginCustomQueryAnswerPacket answer) {
                final CompletableFuture<byte[]> future = this.pending.remove(answer.queryId);
                if (future != null) {
                    future.complete(answer.response);
                    return false;
                }
            }
            return true;
        }

    }

}
