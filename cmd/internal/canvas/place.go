package canvas

// Where a node goes when no one says: beside the node it is made near (or
// the one that made it), on the side asked for first, then right of it,
// below, left and above, gap apart; a place taken — closer than margin to
// another node — moves along the side, step at a time, until it is free.
// When no side has room, the places of a spiral around the node are tried.
// The same canvas gives the same place.

const (
	placeGap    = 40
	placeMargin = 32
	placeStep   = 40
	placeTries  = 60
	gridSize    = 8
)

// sides are the sides tried, in order.
var sides = []string{"right", "below", "left", "above"}

// validSide reports whether side names a side.
func validSide(side string) bool {
	for _, s := range sides {
		if s == side {
			return true
		}
	}
	return false
}

// overlaps reports whether a and b are closer than margin.
func overlaps(a, b Rect, margin int) bool {
	return a.X < b.X+b.W+margin && b.X < a.X+a.W+margin && a.Y < b.Y+b.H+margin && b.Y < a.Y+a.H+margin
}

// free reports whether r keeps margin from every node of taken.
func free(r Rect, taken []Rect) bool {
	for _, other := range taken {
		if overlaps(r, other, placeMargin) {
			return false
		}
	}
	return true
}

func snap(v int) int {
	if v >= 0 {
		return (v + gridSize/2) / gridSize * gridSize
	}
	return -((-v + gridSize/2) / gridSize * gridSize)
}

// bounds is the rectangle around rects.
func bounds(rects []Rect) Rect {
	if len(rects) == 0 {
		return Rect{}
	}
	minX, minY := rects[0].X, rects[0].Y
	maxX, maxY := rects[0].X+rects[0].W, rects[0].Y+rects[0].H
	for _, r := range rects[1:] {
		minX, minY = min(minX, r.X), min(minY, r.Y)
		maxX, maxY = max(maxX, r.X+r.W), max(maxY, r.Y+r.H)
	}
	return Rect{minX, minY, maxX - minX, maxY - minY}
}

// placeNear finds a free place of size w×h beside anchor, side first; with
// no anchor, beside all the nodes there are, or at the origin when there
// are none.
func placeNear(taken []Rect, anchor *Rect, side string, w, h int) Rect {
	if anchor == nil {
		if len(taken) == 0 {
			return Rect{0, 0, w, h}
		}
		box := bounds(taken)
		anchor = &box
		if side == "" {
			side = "right"
		}
	}
	order := make([]string, 0, len(sides))
	if validSide(side) {
		order = append(order, side)
	}
	for _, s := range sides {
		if s != side {
			order = append(order, s)
		}
	}
	for _, s := range order {
		if r, ok := placeBeside(taken, *anchor, s, w, h); ok {
			return r
		}
	}
	return spiral(taken, *anchor, w, h)
}

// placeBeside tries the places on one side of anchor.
func placeBeside(taken []Rect, anchor Rect, side string, w, h int) (Rect, bool) {
	var r Rect
	var dx, dy int
	switch side {
	case "right":
		r, dy = Rect{anchor.X + anchor.W + placeGap, anchor.Y, w, h}, placeStep
	case "left":
		r, dy = Rect{anchor.X - placeGap - w, anchor.Y, w, h}, placeStep
	case "below":
		r, dx = Rect{anchor.X, anchor.Y + anchor.H + placeGap, w, h}, placeStep
	case "above":
		r, dx = Rect{anchor.X, anchor.Y - placeGap - h, w, h}, placeStep
	default:
		return Rect{}, false
	}
	r.X, r.Y = snap(r.X), snap(r.Y)
	for range placeTries {
		if free(r, taken) && within(r) {
			return r, true
		}
		r.X += dx
		r.Y += dy
	}
	return Rect{}, false
}

// spiral tries places on rings around anchor, farther and farther.
func spiral(taken []Rect, anchor Rect, w, h int) Rect {
	cx, cy := anchor.X+anchor.W/2, anchor.Y+anchor.H/2
	unitX, unitY := w+placeGap, h+placeGap
	for ring := 1; ring < 64; ring++ {
		for i := -ring; i <= ring; i++ {
			for _, at := range [][2]int{{i, -ring}, {ring, i}, {-i, ring}, {-ring, -i}} {
				r := Rect{snap(cx + at[0]*unitX - w/2), snap(cy + at[1]*unitY - h/2), w, h}
				if free(r, taken) && within(r) {
					return r
				}
			}
		}
	}
	return Rect{snap(anchor.X), snap(anchor.Y + anchor.H + placeGap), w, h}
}

// within reports whether a place is inside the board.
func within(r Rect) bool {
	return r.X > -maxCoordinate && r.Y > -maxCoordinate && r.X+r.W < maxCoordinate && r.Y+r.H < maxCoordinate
}

// shiftFree moves a place someone gave until it is free: down, step at a
// time, then on as placeNear would.
func shiftFree(taken []Rect, r Rect) Rect {
	if free(r, taken) {
		return r
	}
	at := r
	for range placeTries {
		at.Y += placeStep
		if free(at, taken) && within(at) {
			return at
		}
	}
	return spiral(taken, r, r.W, r.H)
}
