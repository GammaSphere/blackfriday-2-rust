// Package blackfriday presents upstream blackfriday v2's API, implemented by
// the Rust port.
//
// It exists so the test suite in tests/original/ -- which is byte-identical to
// upstream's, and verified so by `make verify-hashes` -- can be compiled and
// run without a single edit. Every exported name here matches upstream's
// signature; the bodies marshal to the `bf-serve` helper over a pipe and
// unmarshal the answer.
//
// Two unexported names are included as well, escapeHTML and isFenceLine,
// because the pinned suite calls them directly. They are the only unexported
// identifiers it reaches.
//
// See harness/src/main.rs for why this is a pipe rather than cgo, and for the
// wire format.
package blackfriday

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
)

// Version is the library version, matching upstream's constant.
const Version = "2.0"

// Extensions is a bitmask of enabled parser extensions.
type Extensions int

// The available parser extensions. Bit 0 is unused: NoExtensions occupies
// iota 0 in upstream's const block, so the first real flag is 1 << 1.
const (
	NoExtensions           Extensions = 0
	NoIntraEmphasis        Extensions = 1 << iota // ignore emphasis markers inside words
	Tables                                        // parse tables
	FencedCode                                    // parse fenced code blocks
	Autolink                                      // detect embedded URLs unenclosed in brackets
	Strikethrough                                 // strikethrough text with two tildes
	LaxHTMLBlocks                                 // loosen up HTML block parsing rules
	SpaceHeadings                                 // be strict about prefix heading rules
	HardLineBreak                                 // translate newlines into line breaks
	TabSizeEight                                  // expand tabs to eight spaces instead of four
	Footnotes                                     // Pandoc-style footnotes
	NoEmptyLineBeforeBlock                        // no need to insert an empty line to start a new paragraph
	HeadingIDs                                    // specify heading IDs with {#id}
	Titleblock                                    // Titleblock ala pandoc
	AutoHeadingIDs                                // create the heading ID from the text
	BackslashLineBreak                            // translate trailing backslashes into line breaks
	DefinitionLists                               // render definition lists

	CommonHTMLFlags HTMLFlags = UseXHTML | Smartypants |
		SmartypantsFractions | SmartypantsDashes | SmartypantsLatexDashes

	CommonExtensions Extensions = NoIntraEmphasis | Tables | FencedCode |
		Autolink | Strikethrough | SpaceHeadings | HeadingIDs |
		BackslashLineBreak | DefinitionLists
)

// HTMLFlags controls the HTML renderer.
type HTMLFlags int

// The available renderer flags. Bit 0 is unused, as with Extensions.
const (
	HTMLFlagsNone       HTMLFlags = 0
	SkipHTML            HTMLFlags = 1 << iota // skip preformatted HTML blocks
	SkipImages                                // skip embedded images
	SkipLinks                                 // skip all links
	Safelink                                  // only link to trusted protocols
	NofollowLinks                             // only link with rel="nofollow"
	NoreferrerLinks                           // only link with rel="noreferrer"
	NoopenerLinks                             // only link with rel="noopener"
	HrefTargetBlank                           // add a blank target
	CompletePage                              // generate a complete HTML page
	UseXHTML                                  // generate XHTML output instead of HTML
	FootnoteReturnLinks                       // generate a link at the end of a footnote to return to the source
	Smartypants                               // enable smart punctuation substitutions
	SmartypantsFractions                      // enable smart fractions
	SmartypantsDashes                         // enable smart dashes
	SmartypantsLatexDashes                    // enable LaTeX-style dashes
	SmartypantsAngledQuotes                   // enable angled double quotes
	SmartypantsQuotesNBSP                     // enable French guillemets
	TOC                                       // generate a table of contents
)

// Reference represents the details of a link.
type Reference struct {
	// Link is usually the URL the reference points to.
	Link string
	// Title is the alternate text describing the link in more detail.
	Title string
	// Text is the optional text to override the ref with.
	Text string
}

// ReferenceOverrideFunc is expected to be called with a reference string and
// return either a valid Reference type that the reference string maps to or
// nil. If overridden is false, the default reference logic will be executed.
type ReferenceOverrideFunc func(reference string) (ref *Reference, overridden bool)

// HTMLRendererParameters holds the renderer's configuration.
type HTMLRendererParameters struct {
	// Prepend this text to each relative URL.
	AbsolutePrefix string
	// Add this text to each footnote anchor, to ensure uniqueness.
	FootnoteAnchorPrefix string
	// Show this text inside the <a> tag for a footnote return link.
	FootnoteReturnLinkContents string
	// Add this prefix to each heading ID, to ensure uniqueness.
	HeadingIDPrefix string
	// Add this suffix to each heading ID, to ensure uniqueness.
	HeadingIDSuffix string
	// Increase heading levels: if the offset is 1, <h1> becomes <h2>.
	HeadingLevelOffset int

	Title string // document title (used if CompletePage is set)
	CSS   string // optional CSS file URL (used if CompletePage is set)
	Icon  string // optional icon file URL (used if CompletePage is set)

	Flags HTMLFlags // flags allow customizing this renderer's behavior
}

// Renderer is the rendering interface.
//
// The pinned suite only ever constructs an *HTMLRenderer and hands it to
// WithRenderer, so this adapter carries the type without the method set: the
// rendering happens in Rust, and a Go-side custom renderer has no way to
// participate. Run reports that rather than rendering something wrong.
type Renderer interface {
	isBlackfridayRenderer()
}

// HTMLRenderer is a type that implements the Renderer interface for HTML.
type HTMLRenderer struct {
	HTMLRendererParameters
}

func (r *HTMLRenderer) isBlackfridayRenderer() {}

// NewHTMLRenderer creates and configures an HTMLRenderer object.
func NewHTMLRenderer(params HTMLRendererParameters) *HTMLRenderer {
	return &HTMLRenderer{HTMLRendererParameters: params}
}

// Option customises the parser.
type Option func(*config)

type config struct {
	extensions        Extensions
	renderer          *HTMLRenderer
	referenceOverride ReferenceOverrideFunc
}

// WithRenderer allows you to override the default renderer.
func WithRenderer(r Renderer) Option {
	return func(c *config) {
		if hr, ok := r.(*HTMLRenderer); ok {
			c.renderer = hr
			return
		}
		panic("blackfriday adapter: only *HTMLRenderer is supported; " +
			"rendering happens in the Rust port, so a Go-side custom renderer " +
			"cannot participate")
	}
}

// WithExtensions allows you to pick some of the many extensions provided by
// Blackfriday.
func WithExtensions(e Extensions) Option {
	return func(c *config) { c.extensions = e }
}

// WithNoExtensions turns off all extensions and custom behavior.
func WithNoExtensions() Option {
	return func(c *config) {
		c.extensions = NoExtensions
		c.renderer = NewHTMLRenderer(HTMLRendererParameters{Flags: HTMLFlagsNone})
	}
}

// WithRefOverride sets an optional function callback that is called every time
// a reference is resolved.
func WithRefOverride(o ReferenceOverrideFunc) Option {
	return func(c *config) { c.referenceOverride = o }
}

// Run is the main entry point to Blackfriday. It parses and renders a block of
// markdown-encoded text.
func Run(input []byte, opts ...Option) []byte {
	c := &config{
		extensions: CommonExtensions,
		renderer:   NewHTMLRenderer(HTMLRendererParameters{Flags: CommonHTMLFlags}),
	}
	for _, opt := range opts {
		if opt != nil {
			opt(c)
		}
	}

	p := c.renderer.HTMLRendererParameters
	args := [][]byte{
		input,
		i32le(int32(c.extensions)),
		i32le(int32(p.Flags)),
		i32le(int32(p.HeadingLevelOffset)),
		[]byte(p.AbsolutePrefix),
		[]byte(p.FootnoteAnchorPrefix),
		[]byte(p.FootnoteReturnLinkContents),
		[]byte(p.HeadingIDPrefix),
		[]byte(p.HeadingIDSuffix),
		[]byte(p.Title),
		[]byte(p.CSS),
		[]byte(p.Icon),
		boolByte(c.referenceOverride != nil),
	}

	vals := call(opRun, args, c.referenceOverride)
	return vals[0]
}

// SanitizedAnchorName returns a sanitized anchor name for the given text.
func SanitizedAnchorName(text string) string {
	vals := call(opSanitizedAnchorName, [][]byte{[]byte(text)}, nil)
	return string(vals[0])
}

// escapeHTML is unexported upstream; esc_test.go calls it directly.
func escapeHTML(w io.Writer, s []byte) {
	vals := call(opEscapeHTML, [][]byte{s}, nil)
	w.Write(vals[0])
}

// isFenceLine is unexported upstream; block_test.go calls it directly.
//
// A nil info means "do not extract an info string", exactly as a nil *string
// does upstream.
func isFenceLine(data []byte, info *string, oldmarker string) (end int, marker string) {
	vals := call(opIsFenceLine, [][]byte{
		data, []byte(oldmarker), boolByte(info != nil),
	}, nil)
	end = int(binary.LittleEndian.Uint64(vals[0]))
	marker = string(vals[1])
	if info != nil {
		*info = string(vals[2])
	}
	return end, marker
}

// ---------------------------------------------------------------------------
// The pipe
// ---------------------------------------------------------------------------

const (
	opRun                = 1
	opEscapeHTML         = 2
	opIsFenceLine        = 3
	opSanitizedAnchorName = 4
	opVersion            = 5

	statusResult  = 0
	statusNeedRef = 1
)

var (
	// One helper process for the whole test binary, started on first use.
	// The suite marks tests Parallel, so every exchange is serialised: the
	// protocol is a strict request/response on one pipe and has no room for
	// interleaving.
	serveOnce sync.Once
	serveMu   sync.Mutex
	serveIn   io.WriteCloser
	serveOut  io.Reader
	serveCmd  *exec.Cmd
	serveErr  error
)

// serverPath locates bf-serve, preferring an explicit override.
func serverPath() (string, error) {
	if p := os.Getenv("BF_SERVE"); p != "" {
		return p, nil
	}
	exe := "bf-serve"
	if os.PathSeparator == '\\' {
		exe = "bf-serve.exe"
	}
	// The adapter lives at <repo>/adapter, the binary at <repo>/target/....
	for _, dir := range []string{
		filepath.Join("..", "target", "release"),
		filepath.Join("..", "target", "debug"),
		filepath.Join("..", "..", "target", "release"),
	} {
		p := filepath.Join(dir, exe)
		if _, err := os.Stat(p); err == nil {
			return filepath.Abs(p)
		}
	}
	return "", fmt.Errorf("bf-serve not found: run `cargo build --release -p blackfriday-harness`, "+
		"or set BF_SERVE to its path (looked for %q)", exe)
}

func startServer() {
	path, err := serverPath()
	if err != nil {
		serveErr = err
		return
	}
	cmd := exec.Command(path)
	cmd.Stderr = os.Stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		serveErr = err
		return
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		serveErr = err
		return
	}
	if err := cmd.Start(); err != nil {
		serveErr = err
		return
	}
	serveCmd, serveIn, serveOut = cmd, stdin, stdout
}

// call performs one exchange, answering any override requests along the way.
func call(op byte, args [][]byte, override ReferenceOverrideFunc) [][]byte {
	serveOnce.Do(startServer)
	if serveErr != nil {
		panic(serveErr)
	}

	serveMu.Lock()
	defer serveMu.Unlock()

	if err := writeFrame(serveIn, op, args); err != nil {
		panic(err)
	}

	for {
		status, vals, err := readFrame(serveOut)
		if err != nil {
			panic(err)
		}
		switch status {
		case statusResult:
			return vals
		case statusNeedRef:
			reply := overrideReply(override, string(vals[0]))
			if err := writeFrame(serveIn, 0, reply); err != nil {
				panic(err)
			}
		default:
			panic(fmt.Sprintf("blackfriday adapter: unknown status %d", status))
		}
	}
}

// overrideReply encodes what the callback said, preserving the three-way
// distinction upstream's (ref, overridden) pair carries.
func overrideReply(override ReferenceOverrideFunc, id string) [][]byte {
	if override == nil {
		return [][]byte{{0}, nil, nil, nil}
	}
	ref, overridden := override(id)
	switch {
	case !overridden:
		return [][]byte{{0}, nil, nil, nil}
	case ref == nil:
		return [][]byte{{2}, nil, nil, nil}
	default:
		return [][]byte{{1}, []byte(ref.Link), []byte(ref.Title), []byte(ref.Text)}
	}
}

func writeFrame(w io.Writer, tag byte, vals [][]byte) error {
	var buf bytes.Buffer
	buf.WriteByte(tag)
	var n [4]byte
	binary.LittleEndian.PutUint32(n[:], uint32(len(vals)))
	buf.Write(n[:])
	for _, v := range vals {
		binary.LittleEndian.PutUint32(n[:], uint32(len(v)))
		buf.Write(n[:])
		buf.Write(v)
	}
	_, err := w.Write(buf.Bytes())
	return err
}

func readFrame(r io.Reader) (byte, [][]byte, error) {
	var tag [1]byte
	if _, err := io.ReadFull(r, tag[:]); err != nil {
		return 0, nil, err
	}
	count, err := readU32(r)
	if err != nil {
		return 0, nil, err
	}
	vals := make([][]byte, count)
	for i := range vals {
		size, err := readU32(r)
		if err != nil {
			return 0, nil, err
		}
		buf := make([]byte, size)
		if _, err := io.ReadFull(r, buf); err != nil {
			return 0, nil, err
		}
		vals[i] = buf
	}
	return tag[0], vals, nil
}

func readU32(r io.Reader) (uint32, error) {
	var b [4]byte
	if _, err := io.ReadFull(r, b[:]); err != nil {
		return 0, err
	}
	return binary.LittleEndian.Uint32(b[:]), nil
}

func i32le(v int32) []byte {
	b := make([]byte, 4)
	binary.LittleEndian.PutUint32(b, uint32(v))
	return b
}

func boolByte(b bool) []byte {
	if b {
		return []byte{1}
	}
	return []byte{0}
}

// Shutdown stops the helper process. Nothing in the suite calls it; it exists
// so a caller embedding this adapter can release the child deliberately.
func Shutdown() {
	serveMu.Lock()
	defer serveMu.Unlock()
	if serveIn != nil {
		serveIn.Close()
		serveIn = nil
	}
	if serveCmd != nil {
		serveCmd.Wait()
		serveCmd = nil
	}
}
