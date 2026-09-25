package main

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"image"
	"image/color"
	"io"
	"math"
	"math/rand/v2"
	"strconv"
	"strings"
	"time"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/charmbracelet/x/ansi/kitty"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/muesli/termenv"
)

// Pictures on screen. Terminals that speak the kitty graphics protocol
// (kitty, Ghostty) draw images: each is transmitted once, and a virtual
// placement says how many cells it covers. The screen then only holds text,
// cells of the Unicode placeholder U+10EEEE whose colour is the image's ID
// and whose diacritics name the row and column of the image they show. To
// Bubble Tea those cells are text like any other, so it draws, moves and
// erases the pictures with the rest of the screen. Other terminals get the
// picture in half blocks, two pixels a cell, in true colour; and without
// colours, a description.

type graphicsMode int

const (
	graphicsText   graphicsMode = iota // descriptions only
	graphicsBlocks                     // half blocks in true colour
	graphicsKitty                      // the kitty graphics protocol
)

// graphicsEnvironment overrides the detection: kitty, blocks or text.
const graphicsEnvironment = "KOU_CONVEYOR_IMAGES"

// detectGraphics picks how the terminal shows pictures.
func detectGraphics(getenv func(string) string, noColor bool) (graphicsMode, bool) {
	tmux := getenv("TMUX") != ""
	switch strings.ToLower(strings.TrimSpace(getenv(graphicsEnvironment))) {
	case "kitty":
		return graphicsKitty, tmux
	case "blocks":
		return graphicsBlocks, false
	case "text", "off", "none":
		return graphicsText, false
	}
	if noColor {
		return graphicsText, false
	}
	term, program := getenv("TERM"), strings.ToLower(getenv("TERM_PROGRAM"))
	// Inside tmux or screen, sequences need passthrough the user may not
	// have allowed.
	if !tmux && !strings.HasPrefix(term, "screen") {
		switch {
		case getenv("KITTY_WINDOW_ID") != "", term == "xterm-kitty",
			term == "xterm-ghostty", program == "ghostty":
			return graphicsKitty, false
		}
	}
	if lipgloss.ColorProfile() == termenv.TrueColor {
		return graphicsBlocks, false
	}
	return graphicsText, false
}

// graphics sends images to the terminal.
type graphics struct {
	mode graphicsMode
	tmux bool // wrap sequences for tmux passthrough
	// cellW and cellH are a cell's pixels; their ratio keeps pictures in
	// proportion. Terminals that do not say get 1:2.
	cellW, cellH int
	next         uint32
	// out takes sequences ([]byte) for the writer goroutine, which writes
	// each whole: a write to the terminal is never split by one of the
	// renderer's. A channel in it is closed once what came before is
	// written. With no goroutine, sequences go to w at once.
	out  chan any
	w    io.Writer
	done chan struct{}
}

func newGraphics(mode graphicsMode, tmux bool, w io.Writer) *graphics {
	return &graphics{mode: mode, tmux: tmux, w: w, cellW: 10, cellH: 20,
		// IDs are the placeholders' 24-bit colours; a random start keeps
		// clear of images another program left in the terminal.
		next: 1<<20 + rand.Uint32N(1<<23)}
}

// start writes sequences from a goroutine of their own, so that a large
// image never holds up the screen.
func (g *graphics) start() {
	g.out, g.done = make(chan any, 64), make(chan struct{})
	go func() {
		defer close(g.done)
		for item := range g.out {
			switch item := item.(type) {
			case []byte:
				g.w.Write(item)
			case chan struct{}:
				close(item)
			}
		}
	}()
}

// stop waits a while for what was sent to be written, and ends the writer.
func (g *graphics) stop(timeout time.Duration) {
	if g.out == nil {
		return
	}
	close(g.out)
	select {
	case <-g.done:
	case <-time.After(timeout):
	}
	g.out = nil
}

// drain waits a while for what was sent to be written: switching screens
// must come after the deletions meant for the screen that is left.
func (g *graphics) drain(timeout time.Duration) {
	if g.out == nil {
		return
	}
	written := make(chan struct{})
	deadline := time.After(timeout)
	select {
	case g.out <- written:
	case <-deadline:
		return
	}
	select {
	case <-written:
	case <-deadline:
	}
}

func (g *graphics) emit(seq []byte) {
	if len(seq) == 0 {
		return
	}
	if g.out != nil {
		g.out <- seq
		return
	}
	g.w.Write(seq)
}

// command is one sequence of the protocol, wrapped for tmux when needed.
func (g *graphics) command(payload []byte, options string) string {
	seq := ansi.KittyGraphics(payload, strings.Split(options, ",")...)
	if g.tmux {
		return ansi.TmuxPassthrough(seq)
	}
	return seq
}

// measure takes the cell size the terminal reports.
func (g *graphics) measure(width, height int) {
	if width > 0 && height > 0 {
		g.cellW, g.cellH = width, height
	}
}

// fit is how many cells a width×height picture covers at most cols×rows
// cells, in proportion; never more than twice its own size.
func (g *graphics) fit(width, height, cols, rows int) (int, int) {
	if width <= 0 || height <= 0 || cols <= 0 || rows <= 0 {
		return 0, 0
	}
	w, h := float64(width)/float64(g.cellW), float64(height)/float64(g.cellH) // in cells
	scale := min(float64(cols)/w, float64(rows)/h, 2)
	return clamp(int(math.Round(w*scale)), 1, cols), clamp(int(math.Round(h*scale)), 1, rows)
}

// picture is an image as the terminal shows it: a PNG for the protocol, and
// a small copy for half blocks.
type picture struct {
	png    []byte
	small  image.Image
	width  int // of png
	height int
	// id is the image's ID once transmitted, and cols×rows the size of its
	// virtual placement.
	id         uint32
	cols, rows int
	// blocks caches the half blocks drawn last.
	blocks      []string
	blocksSize  [2]int
	blocksWidth int
}

// place transmits the picture if the terminal does not have it, and gives
// it a virtual placement of cols×rows cells.
func (g *graphics) place(p *picture, cols, rows int) {
	if g.mode != graphicsKitty || p == nil || len(p.png) == 0 {
		return
	}
	var seq bytes.Buffer
	if p.id == 0 {
		p.id, p.cols, p.rows = g.next, 0, 0
		g.next = g.next%(1<<24-1) + 1
		encoded := base64.StdEncoding.EncodeToString(p.png)
		for i := 0; i < len(encoded); i += kitty.MaxChunkSize {
			chunk := encoded[i:min(i+kitty.MaxChunkSize, len(encoded))]
			more := 0
			if i+kitty.MaxChunkSize < len(encoded) {
				more = 1
			}
			options := "q=2,m=" + strconv.Itoa(more)
			if i == 0 {
				options = fmt.Sprintf("a=t,f=100,t=d,i=%d,q=2,m=%d", p.id, more)
			}
			seq.WriteString(g.command([]byte(chunk), options))
		}
	}
	if p.cols != cols || p.rows != rows {
		p.cols, p.rows = cols, rows
		seq.WriteString(g.command(nil, fmt.Sprintf("a=p,U=1,i=%d,p=1,c=%d,r=%d,q=2", p.id, cols, rows)))
	}
	g.emit(seq.Bytes())
}

// forget deletes the picture from the terminal.
func (g *graphics) forget(p *picture) {
	if p == nil || p.id == 0 {
		return
	}
	if g.mode == graphicsKitty {
		g.emit([]byte(g.command(nil, fmt.Sprintf("a=d,d=I,i=%d,q=2", p.id))))
	}
	p.id, p.cols, p.rows = 0, 0, 0
}

// lost marks the picture as one the terminal no longer has: switching
// screens leaves a screen's images behind.
func (p *picture) lost() {
	if p != nil {
		p.id, p.cols, p.rows = 0, 0, 0
	}
}

// draw renders the picture in cols×rows cells, one string a row; place
// must have been called for the size. Without pictures, it has no rows.
func (g *graphics) draw(p *picture, cols, rows int) []string {
	if p == nil || cols <= 0 || rows <= 0 {
		return nil
	}
	switch g.mode {
	case graphicsKitty:
		if p.id == 0 {
			return nil
		}
		// The foreground colour is the image's ID.
		color := fmt.Sprintf("\x1b[38;2;%d;%d;%dm", p.id>>16&0xff, p.id>>8&0xff, p.id&0xff)
		lines := make([]string, rows)
		for row := range rows {
			var b strings.Builder
			b.WriteString(color)
			for col := range cols {
				b.WriteRune(kitty.Placeholder)
				b.WriteRune(kitty.Diacritic(row))
				b.WriteRune(kitty.Diacritic(col))
			}
			b.WriteString("\x1b[39m")
			lines[row] = b.String()
		}
		return lines
	case graphicsBlocks:
		return p.halfBlocks(cols, rows)
	}
	return nil
}

// halfBlocks draws the picture two pixels a cell: the upper half block in the
// colour of the upper pixel over the colour of the lower one.
func (p *picture) halfBlocks(cols, rows int) []string {
	if p.small == nil {
		return nil
	}
	if p.blocksSize == [2]int{cols, rows} {
		return p.blocks
	}
	scaled := cockpit.Scale(p.small, cols, rows*2)
	lines := make([]string, rows)
	for row := range rows {
		var b strings.Builder
		for col := range cols {
			top, bottom := rgb(scaled.At(col, row*2)), rgb(scaled.At(col, row*2+1))
			fmt.Fprintf(&b, "\x1b[38;2;%d;%d;%d;48;2;%d;%d;%dm▀", top[0], top[1], top[2], bottom[0], bottom[1], bottom[2])
		}
		b.WriteString("\x1b[m")
		lines[row] = b.String()
	}
	p.blocks, p.blocksSize = lines, [2]int{cols, rows}
	return lines
}

// blockBackground is what transparent pixels are laid over in half blocks,
// in 16 bits: dark or light, like the terminal.
var blockBackground uint32 = 0x1a1a

// rgb is a colour laid over blockBackground by its alpha.
func rgb(c color.Color) [3]uint8 {
	r, g, b, a := c.RGBA() // premultiplied
	over := func(v uint32) uint8 { return uint8((v + blockBackground*(0xffff-a)/0xffff) >> 8) }
	return [3]uint8{over(r), over(g), over(b)}
}
