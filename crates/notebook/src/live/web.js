const sockets = new Map();
let next = 1;

export function liveConnect(url, message, closed) {
  const id = next++;
  const socket = new WebSocket(url);
  socket.binaryType = 'arraybuffer';
  socket.onmessage = event => message(event.data);
  socket.onclose = () => {
    liveClose(id);
    closed();
  };
  sockets.set(id, socket);
  return id;
}

export function liveSend(id, bytes) {
  const socket = sockets.get(id);
  if (!socket || socket.readyState !== WebSocket.OPEN) throw new Error('Relay disconnected');
  if (socket.bufferedAmount > 1048576) throw new Error('Relay cannot keep up');
  socket.send(bytes);
}

export function liveClose(id) {
  const socket = sockets.get(id);
  sockets.delete(id);
  if (!socket) return;
  socket.onmessage = socket.onclose = null;
  socket.close();
}
