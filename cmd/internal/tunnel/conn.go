package tunnel

import (
	"errors"
	"io"
	"net"
	"sync"
	"time"

	"github.com/gorilla/websocket"
	"github.com/hashicorp/yamux"
)

// writeTimeout bounds a write to the relay's WebSocket: a connection that
// takes longer is dead, and the session over it ends.
const writeTimeout = 30 * time.Second

// wsConn is a WebSocket as the stream of bytes yamux runs on: each write a
// binary message, the messages read back to back.
type wsConn struct {
	ws     *websocket.Conn
	reader io.Reader
	write  sync.Mutex
}

func newConn(ws *websocket.Conn) *wsConn { return &wsConn{ws: ws} }

func (c *wsConn) Read(p []byte) (int, error) {
	for {
		if c.reader == nil {
			kind, reader, err := c.ws.NextReader()
			if err != nil {
				return 0, err
			}
			if kind != websocket.BinaryMessage {
				continue // not the session's
			}
			c.reader = reader
		}
		n, err := c.reader.Read(p)
		if errors.Is(err, io.EOF) {
			c.reader = nil
			if n == 0 {
				continue
			}
			err = nil
		}
		return n, err
	}
}

func (c *wsConn) Write(p []byte) (int, error) {
	c.write.Lock()
	defer c.write.Unlock()
	c.ws.SetWriteDeadline(time.Now().Add(writeTimeout))
	if err := c.ws.WriteMessage(websocket.BinaryMessage, p); err != nil {
		return 0, err
	}
	return len(p), nil
}

func (c *wsConn) Close() error         { return c.ws.Close() }
func (c *wsConn) LocalAddr() net.Addr  { return c.ws.LocalAddr() }
func (c *wsConn) RemoteAddr() net.Addr { return c.ws.RemoteAddr() }

// sessionConfig is how both ends run yamux: pings often enough that a
// connection lost on the way is noticed in half a minute, and wide windows,
// for files and event streams.
func sessionConfig() *yamux.Config {
	c := yamux.DefaultConfig()
	c.KeepAliveInterval = 15 * time.Second
	c.ConnectionWriteTimeout = 15 * time.Second
	c.MaxStreamWindowSize = 1 << 20
	c.LogOutput = io.Discard // a session that ends says so by ending
	return c
}
