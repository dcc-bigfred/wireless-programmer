package client

import (
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"time"
)

const (
	// DefaultSocket is the daemon socket when DATA_DIR is /data.
	DefaultSocket  = "/data/run/wireless-programmer/wireless-programmer.sock"
	maxFrameBytes  = 1024 * 1024
	defaultTimeout = 10 * time.Second
)

var (
	// ErrBusy is returned when the radio is already in use by another job.
	ErrBusy = errors.New("radio busy")
	// ErrNotFound is returned when the referenced job or candidate does not exist.
	ErrNotFound = errors.New("not found")
	// ErrNoCandidates is returned when a scan found no devices.
	ErrNoCandidates = errors.New("no candidates")
)

// IdentityFormatWire mirrors wp_proto::IdentityFormatWire.
type IdentityFormatWire struct {
	Type   string `json:"type"`
	Len    uint8  `json:"len,omitempty"`
	MaxLen uint8  `json:"maxLen,omitempty"`
}

// CommissioningKindWire mirrors wp_proto::CommissioningKindWire.
type CommissioningKindWire string

// CapabilitiesWire mirrors wp_proto::CapabilitiesWire.
type CapabilitiesWire struct {
	MaxRosterSlots         uint8                 `json:"maxRosterSlots"`
	MaxFunctionIndex       uint8                 `json:"maxFunctionIndex"`
	IdentityFormat         IdentityFormatWire    `json:"identityFormat"`
	SupportsThrottleServer bool                  `json:"supportsThrottleServer"`
	SupportsFirmwareUpdate bool                  `json:"supportsFirmwareUpdate"`
	Commissioning          CommissioningKindWire `json:"commissioning"`
	CommissioningNet       *CommissioningNetWire `json:"commissioningNet,omitempty"`
}

// CommissioningNetWire mirrors wp_proto::CommissioningNetWire.
type CommissioningNetWire struct {
	Host   string `json:"host"`
	Port   uint16 `json:"port"`
	Source string `json:"source"`
	Prefix uint8  `json:"prefix"`
}

// DriverInfoWire mirrors wp_proto::DriverInfoWire.
type DriverInfoWire struct {
	ID           string           `json:"id"`
	Name         string           `json:"name"`
	Capabilities CapabilitiesWire `json:"capabilities"`
}

// HelloResult mirrors wp_proto::HelloResult.
type HelloResult struct {
	Version string           `json:"version"`
	Commit  string           `json:"commit,omitempty"`
	Drivers []DriverInfoWire `json:"drivers"`
}

// CandidateWire mirrors wp_proto::CandidateWire.
type CandidateWire struct {
	Driver string `json:"driver"`
	Key    string `json:"key"`
	Label  string `json:"label"`
	RSSI   *int32 `json:"rssi,omitempty"`
}

// CandidateRef mirrors wp_proto::CandidateRef.
type CandidateRef struct {
	Driver string `json:"driver"`
	Key    string `json:"key"`
}

// WifiCredentialsWire mirrors wp_proto::WifiCredentialsWire.
type WifiCredentialsWire struct {
	SSID string `json:"ssid"`
	PSK  string `json:"psk,omitempty"`
}

// ThrottleServerWire mirrors wp_proto::ThrottleServerWire.
type ThrottleServerWire struct {
	Host      string `json:"host"`
	Port      uint16 `json:"port"`
	Automatic *bool  `json:"automatic,omitempty"`
}

// FunctionMappingWire mirrors wp_proto::FunctionMappingWire.
type FunctionMappingWire struct {
	Index uint8 `json:"index"`
	Value uint8 `json:"value"`
}

// RosterEntryWire mirrors wp_proto::RosterEntryWire.
type RosterEntryWire struct {
	Address     *uint16               `json:"address,omitempty"`
	LongAddress *bool                 `json:"longAddress,omitempty"`
	Mode        string                `json:"mode,omitempty"`
	Direction   *uint8                `json:"direction,omitempty"`
	Functions   []FunctionMappingWire `json:"functions,omitempty"`
}

// ProgramRequestWire mirrors wp_proto::ProgramRequestWire.
type ProgramRequestWire struct {
	Identity   string              `json:"identity"`
	Wifi       WifiCredentialsWire `json:"wifi"`
	Server     ThrottleServerWire  `json:"server"`
	Roster     []RosterEntryWire   `json:"roster"`
	Bigfred    *BigfredCredsWire   `json:"bigfred,omitempty"`
	RosterMode string              `json:"rosterMode,omitempty"`
}

// BigfredCredsWire mirrors wp_proto::BigfredCredsWire.
type BigfredCredsWire struct {
	Login string `json:"login"`
	PIN   string `json:"pin"`
}

// DeviceInfoWire mirrors wp_proto::DeviceInfoWire.
type DeviceInfoWire struct {
	Driver           string            `json:"driver"`
	Key              string            `json:"key"`
	FirmwareRevision string            `json:"firmwareRevision,omitempty"`
	Identity         string            `json:"identity,omitempty"`
	BatteryMV        *uint32           `json:"batteryMv,omitempty"`
	Roster           []RosterEntryWire `json:"roster,omitempty"`
}

// ProgramResult mirrors wp_proto::ProgramResult.
type ProgramResult struct {
	JobID string `json:"jobId"`
}

// JobStateWire mirrors wp_proto::JobStateWire.
type JobStateWire string

// JobSnapshot mirrors wp_proto::JobSnapshot.
type JobSnapshot struct {
	JobID  string       `json:"jobId"`
	State  JobStateWire `json:"state"`
	Driver string       `json:"driver"`
	Key    string       `json:"key"`
	Detail string       `json:"detail,omitempty"`
}

// JobFrame mirrors wp_proto::JobFrame.
type JobFrame struct {
	JobID    string       `json:"jobId"`
	State    JobStateWire `json:"state"`
	Step     string       `json:"step,omitempty"`
	Progress *uint8       `json:"progress,omitempty"`
	Detail   string       `json:"detail,omitempty"`
}

// LinkStatusWire mirrors wp_proto::LinkStatusWire.
type LinkStatusWire struct {
	Busy          bool   `json:"busy"`
	Interface     string `json:"interface,omitempty"`
	RfkillBlocked bool   `json:"rfkillBlocked"`
}

// Response is one framed IPC reply (exported for streaming callers).
type Response struct {
	Type   string          `json:"type"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  *ErrorBody      `json:"error,omitempty"`
}

// ErrorBody mirrors wp_proto::ErrorBody.
type ErrorBody struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

func (e *ErrorBody) Error() string {
	return e.Message
}

type request struct {
	Type   string         `json:"type"`
	Params *requestParams `json:"params,omitempty"`
}

type requestParams struct {
	Candidate      *CandidateRef       `json:"candidate,omitempty"`
	Request        *ProgramRequestWire `json:"request,omitempty"`
	JobID          string              `json:"jobId,omitempty"`
	Count          *uint32             `json:"count,omitempty"`
	Mode           string              `json:"mode,omitempty"`
	Path           string              `json:"path,omitempty"`
	Host           string              `json:"host,omitempty"`
	Port           string              `json:"port,omitempty"`
	PartitionTable string              `json:"partitionTable,omitempty"`
}

// Client dials the wireless-programmer Unix socket.
type Client struct {
	Socket  string
	Timeout time.Duration
	// Dial is overridden in tests.
	Dial func(network, address string, timeout time.Duration) (net.Conn, error)
}

func (c *Client) socketPath() string {
	if c.Socket != "" {
		return c.Socket
	}
	return DefaultSocket
}

func (c *Client) timeout() time.Duration {
	if c.Timeout > 0 {
		return c.Timeout
	}
	return defaultTimeout
}

func (c *Client) dial() (net.Conn, error) {
	dial := c.Dial
	if dial == nil {
		dial = net.DialTimeout
	}
	conn, err := dial("unix", c.socketPath(), c.timeout())
	if err != nil {
		return nil, fmt.Errorf("connect %s: %w (is wireless-programmer running?)", c.socketPath(), err)
	}
	_ = conn.SetDeadline(time.Now().Add(c.timeout()))
	return conn, nil
}

// Hello exchanges version + driver capabilities.
func (c *Client) Hello() (*HelloResult, error) {
	var resp Response
	if err := c.roundTrip(request{Type: "hello"}, &resp); err != nil {
		return nil, err
	}
	if resp.Type == "error" {
		return nil, responseError(resp)
	}
	if resp.Type != "hello" {
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
	var out HelloResult
	if err := json.Unmarshal(resp.Result, &out); err != nil {
		return nil, fmt.Errorf("decode hello: %w", err)
	}
	return &out, nil
}

// Scan enumerates candidate devices on the radio (Soft-AP).
func (c *Client) Scan() ([]CandidateWire, error) {
	return c.ScanMode("ap")
}

// ScanMode enumerates candidates. mode is "ap" (radio Soft-AP), "lan" (mDNS), or "usb".
func (c *Client) ScanMode(mode string) ([]CandidateWire, error) {
	var params *requestParams
	if mode != "" && mode != "ap" {
		params = &requestParams{Mode: mode}
	}
	var resp Response
	if err := c.roundTrip(request{Type: "scan", Params: params}, &resp); err != nil {
		return nil, err
	}
	if resp.Type == "error" {
		return nil, responseError(resp)
	}
	if resp.Type != "scan" {
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
	var out []CandidateWire
	if err := json.Unmarshal(resp.Result, &out); err != nil {
		return nil, fmt.Errorf("decode scan: %w", err)
	}
	return out, nil
}

// UpdateFirmware queues a firmware-upload job (image path on the hub).
// mode is "ap", "lan", or "usb". host is an optional LAN IPv4; port is a USB serial device.
func (c *Client) UpdateFirmware(mode string, candidate *CandidateRef, path, host, port, partitionTable string) (*ProgramResult, error) {
	params := &requestParams{
		Mode:           mode,
		Path:           path,
		Host:           host,
		Port:           port,
		PartitionTable: partitionTable,
		Candidate:      candidate,
	}
	var resp Response
	if err := c.roundTrip(request{Type: "updateFirmware", Params: params}, &resp); err != nil {
		return nil, err
	}
	if resp.Type == "error" {
		return nil, responseError(resp)
	}
	if resp.Type != "updateFirmware" {
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
	var out ProgramResult
	if err := json.Unmarshal(resp.Result, &out); err != nil {
		return nil, fmt.Errorf("decode updateFirmware: %w", err)
	}
	return &out, nil
}

// Probe reads a single candidate's device info.
func (c *Client) Probe(candidate CandidateRef) (*DeviceInfoWire, error) {
	var resp Response
	if err := c.roundTrip(request{Type: "probe", Params: &requestParams{Candidate: &candidate}}, &resp); err != nil {
		return nil, err
	}
	if resp.Type == "error" {
		return nil, responseError(resp)
	}
	if resp.Type != "probe" {
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
	var out DeviceInfoWire
	if err := json.Unmarshal(resp.Result, &out); err != nil {
		return nil, fmt.Errorf("decode probe: %w", err)
	}
	return &out, nil
}

// Program starts a programming job and returns its id.
func (c *Client) Program(candidate CandidateRef, req ProgramRequestWire) (*ProgramResult, error) {
	var resp Response
	if err := c.roundTrip(request{Type: "program", Params: &requestParams{Candidate: &candidate, Request: &req}}, &resp); err != nil {
		return nil, err
	}
	if resp.Type == "error" {
		return nil, responseError(resp)
	}
	if resp.Type != "program" {
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
	var out ProgramResult
	if err := json.Unmarshal(resp.Result, &out); err != nil {
		return nil, fmt.Errorf("decode program: %w", err)
	}
	return &out, nil
}

// JobGet snapshots a job's state.
func (c *Client) JobGet(jobID string) (*JobSnapshot, error) {
	var resp Response
	if err := c.roundTrip(request{Type: "job.get", Params: &requestParams{JobID: jobID}}, &resp); err != nil {
		return nil, err
	}
	if resp.Type == "error" {
		return nil, responseError(resp)
	}
	if resp.Type != "job.get" {
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
	var out JobSnapshot
	if err := json.Unmarshal(resp.Result, &out); err != nil {
		return nil, fmt.Errorf("decode job: %w", err)
	}
	return &out, nil
}

// JobCancel requests cancellation of a running job.
func (c *Client) JobCancel(jobID string) (*JobSnapshot, error) {
	var resp Response
	if err := c.roundTrip(request{Type: "job.cancel", Params: &requestParams{JobID: jobID}}, &resp); err != nil {
		return nil, err
	}
	if resp.Type == "error" {
		return nil, responseError(resp)
	}
	if resp.Type != "job.cancel" {
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
	var out JobSnapshot
	if err := json.Unmarshal(resp.Result, &out); err != nil {
		return nil, fmt.Errorf("decode job: %w", err)
	}
	return &out, nil
}

// Identify blinks a device's LED so an operator can find it.
func (c *Client) Identify(candidate CandidateRef, count *uint32) error {
	var resp Response
	if err := c.roundTrip(request{Type: "identify", Params: &requestParams{Candidate: &candidate, Count: count}}, &resp); err != nil {
		return err
	}
	if resp.Type == "error" {
		return responseError(resp)
	}
	if resp.Type != "identify" {
		return fmt.Errorf("unexpected response type %q", resp.Type)
	}
	return nil
}

// LinkStatus reports radio/link state.
func (c *Client) LinkStatus() (*LinkStatusWire, error) {
	var resp Response
	if err := c.roundTrip(request{Type: "link.status"}, &resp); err != nil {
		return nil, err
	}
	if resp.Type == "error" {
		return nil, responseError(resp)
	}
	if resp.Type != "link.status" {
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
	var out LinkStatusWire
	if err := json.Unmarshal(resp.Result, &out); err != nil {
		return nil, fmt.Errorf("decode link.status: %w", err)
	}
	return &out, nil
}

// JobWatch opens a streaming connection for job progress frames. Caller
// must Close the conn. Use [ReadFrame] to drain frames with an idle deadline.
func (c *Client) JobWatch(jobID string) (net.Conn, error) {
	conn, err := c.dial()
	if err != nil {
		return nil, err
	}
	if err := writeFrame(conn, request{Type: "job.watch", Params: &requestParams{JobID: jobID}}); err != nil {
		_ = conn.Close()
		return nil, err
	}
	_ = conn.SetDeadline(time.Time{})
	return conn, nil
}

// ReadFrame reads one framed response from a streaming connection with a
// per-frame idle read deadline (Client.Timeout, default 10s).
func (c *Client) ReadFrame(conn net.Conn) (Response, error) {
	_ = conn.SetReadDeadline(time.Now().Add(c.timeout()))
	var resp Response
	if err := readFrame(conn, &resp); err != nil {
		return Response{}, err
	}
	return resp, nil
}

func (c *Client) roundTrip(req request, resp *Response) error {
	conn, err := c.dial()
	if err != nil {
		return err
	}
	defer conn.Close()
	if err := writeFrame(conn, req); err != nil {
		return err
	}
	return readFrame(conn, resp)
}

func writeFrame(w io.Writer, msg any) error {
	payload, err := json.Marshal(msg)
	if err != nil {
		return err
	}
	if len(payload) > maxFrameBytes {
		return errors.New("frame too large")
	}
	var hdr [4]byte
	binary.LittleEndian.PutUint32(hdr[:], uint32(len(payload)))
	if err := writeFull(w, hdr[:]); err != nil {
		return err
	}
	return writeFull(w, payload)
}

func writeFull(w io.Writer, p []byte) error {
	for len(p) > 0 {
		n, err := w.Write(p)
		if n > 0 {
			p = p[n:]
		}
		if err != nil {
			return err
		}
		if n == 0 {
			return io.ErrShortWrite
		}
	}
	return nil
}

func readFrame(r io.Reader, dest any) error {
	var hdr [4]byte
	if _, err := io.ReadFull(r, hdr[:]); err != nil {
		return err
	}
	n := binary.LittleEndian.Uint32(hdr[:])
	if n > maxFrameBytes {
		return fmt.Errorf("frame length %d too large", n)
	}
	buf := make([]byte, n)
	if _, err := io.ReadFull(r, buf); err != nil {
		return err
	}
	return json.Unmarshal(buf, dest)
}

// responseError maps an IPC error response to a typed error.
func responseError(resp Response) error {
	if resp.Error == nil {
		return errors.New("wireless-programmer request failed")
	}
	switch resp.Error.Code {
	case "busy":
		return fmt.Errorf("%s: %w", resp.Error.Message, ErrBusy)
	case "notFound", "candidateNotFound":
		return fmt.Errorf("%s: %w", resp.Error.Message, ErrNotFound)
	case "noCandidates":
		return fmt.Errorf("%s: %w", resp.Error.Message, ErrNoCandidates)
	}
	return resp.Error
}
