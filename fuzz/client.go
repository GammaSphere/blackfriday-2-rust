package main

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"time"
)

// errHang is returned when an implementation stopped making progress.
//
// This is the whole reason both sides run behind a pipe. Neither Go nor Rust
// can interrupt a wedged computation from inside the same process, so
// "does this input finish" is a question only a supervisor holding a killable
// process can answer.
var errHang = errors.New("no response within the time limit")

const (
	statusResult  = 0
	statusNeedRef = 1
	statusPanic   = 2
)

// impl is one implementation, running as a supervised child process.
type impl struct {
	name    string
	path    string
	timeout time.Duration

	cmd  *exec.Cmd
	in   io.WriteCloser
	out  io.Reader
	dead bool
}

func newImpl(name, path string, timeout time.Duration) (*impl, error) {
	i := &impl{name: name, path: path, timeout: timeout}
	return i, i.start()
}

func (i *impl) start() error {
	cmd := exec.Command(i.path)
	cmd.Stderr = nil
	in, err := cmd.StdinPipe()
	if err != nil {
		return err
	}
	out, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	i.cmd, i.in, i.out, i.dead = cmd, in, out, false
	return nil
}

// restart kills the child and starts a fresh one.
//
// Called after a hang or a protocol error, because a child that stopped
// answering has also lost its place in the frame stream — there is no way to
// resynchronise a half-read response, and no reason to try.
func (i *impl) restart() error {
	if i.cmd != nil && i.cmd.Process != nil {
		_ = i.cmd.Process.Kill()
		_ = i.cmd.Wait()
	}
	return i.start()
}

// render sends one request and waits for the answer, or gives up.
//
// A panic reported by the child comes back as a message rather than an error:
// both implementations panic on some inputs deliberately, and agreeing about
// which ones is part of what is being tested.
func (i *impl) render(req [][]byte) (out []byte, panicked string, err error) {
	if i.dead {
		if err := i.restart(); err != nil {
			return nil, "", err
		}
	}

	type reply struct {
		out       []byte
		panicked  string
		err       error
	}
	done := make(chan reply, 1)

	go func() {
		if err := writeFrame(i.in, 1, req); err != nil {
			done <- reply{err: err}
			return
		}
		for {
			status, vals, err := readFrame(i.out)
			if err != nil {
				done <- reply{err: err}
				return
			}
			switch status {
			case statusResult:
				done <- reply{out: vals[0]}
				return
			case statusPanic:
				done <- reply{panicked: string(vals[0])}
				return
			case statusNeedRef:
				// No configuration here uses a reference override, so the
				// only correct answer is "not overridden".
				if err := writeFrame(i.in, 0, [][]byte{{0}, nil, nil, nil}); err != nil {
					done <- reply{err: err}
					return
				}
			default:
				done <- reply{err: fmt.Errorf("unknown status %d", status)}
				return
			}
		}
	}()

	select {
	case r := <-done:
		if r.err != nil {
			i.dead = true
		}
		return r.out, r.panicked, r.err
	case <-time.After(i.timeout):
		// The reader goroutine is still blocked on a pipe that will never
		// produce anything. Killing the child unblocks it, and the child is
		// replaced before the next request.
		i.dead = true
		return nil, "", errHang
	}
}

func (i *impl) close() {
	if i.in != nil {
		i.in.Close()
	}
	if i.cmd != nil && i.cmd.Process != nil {
		_ = i.cmd.Process.Kill()
		_ = i.cmd.Wait()
	}
}

// buildRequest lays out the arguments bf-serve and goserve both expect.
func buildRequest(input []byte, c config) [][]byte {
	return [][]byte{
		input,
		le32(int32(c.ext)),
		le32(int32(c.params.Flags)),
		le32(int32(c.params.HeadingLevelOffset)),
		[]byte(c.params.AbsolutePrefix),
		[]byte(c.params.FootnoteAnchorPrefix),
		[]byte(c.params.FootnoteReturnLinkContents),
		[]byte(c.params.HeadingIDPrefix),
		[]byte(c.params.HeadingIDSuffix),
		[]byte(c.params.Title),
		[]byte(c.params.CSS),
		[]byte(c.params.Icon),
		{0}, // no reference override
	}
}

func le32(v int32) []byte {
	b := make([]byte, 4)
	binary.LittleEndian.PutUint32(b, uint32(v))
	return b
}

func writeFrame(w io.Writer, tag byte, vals [][]byte) error {
	var n [4]byte
	buf := []byte{tag}
	binary.LittleEndian.PutUint32(n[:], uint32(len(vals)))
	buf = append(buf, n[:]...)
	for _, v := range vals {
		binary.LittleEndian.PutUint32(n[:], uint32(len(v)))
		buf = append(buf, n[:]...)
		buf = append(buf, v...)
	}
	_, err := w.Write(buf)
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

// serverPath resolves a helper binary, allowing an override from the
// environment so a release build can be pointed at explicitly.
func serverPath(envVar, def string) (string, error) {
	if p := os.Getenv(envVar); p != "" {
		return p, nil
	}
	if _, err := os.Stat(def); err != nil {
		return "", fmt.Errorf("%s not found (set %s): %w", def, envVar, err)
	}
	// exec.Command resolves a bare name through PATH, never the working
	// directory, so a relative path has to be made absolute here.
	return filepath.Abs(def)
}
