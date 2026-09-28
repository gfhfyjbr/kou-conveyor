package terminal

// ring keeps the last bytes a shell printed, for a page that attaches to it
// to draw the screen from. What falls out of it goes to evicted, which
// follows the terminal modes as they were where the ring now starts.
type ring struct {
	buf   []byte
	start int // where the oldest byte is
	size  int // how many bytes it holds
	// evicted is told of the bytes that leave the ring, oldest first.
	evicted func([]byte)
}

func newRing(capacity int, evicted func([]byte)) *ring {
	return &ring{buf: make([]byte, capacity), evicted: evicted}
}

// write appends p, dropping the oldest bytes when the ring is full.
func (r *ring) write(p []byte) {
	capacity := len(r.buf)
	if len(p) >= capacity {
		// All that was kept leaves, and the start of p with it.
		r.drop(r.size)
		if r.evicted != nil && len(p) > capacity {
			r.evicted(p[:len(p)-capacity])
		}
		copy(r.buf, p[len(p)-capacity:])
		r.start, r.size = 0, capacity
		return
	}
	if over := r.size + len(p) - capacity; over > 0 {
		r.drop(over)
	}
	end := (r.start + r.size) % capacity
	n := copy(r.buf[end:], p)
	copy(r.buf, p[n:])
	r.size += len(p)
}

// drop takes the n oldest bytes out.
func (r *ring) drop(n int) {
	if n <= 0 {
		return
	}
	capacity := len(r.buf)
	if r.evicted != nil {
		first := min(n, capacity-r.start)
		r.evicted(r.buf[r.start : r.start+first])
		if n > first {
			r.evicted(r.buf[:n-first])
		}
	}
	r.start = (r.start + n) % capacity
	r.size -= n
	if r.size == 0 {
		r.start = 0
	}
}

// bytes returns a copy of what the ring holds, oldest first.
func (r *ring) bytes() []byte {
	out := make([]byte, r.size)
	n := copy(out, r.buf[r.start:min(r.start+r.size, len(r.buf))])
	copy(out[n:], r.buf[:r.size-n])
	return out
}

func (r *ring) len() int { return r.size }
