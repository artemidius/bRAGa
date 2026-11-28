// SPDX-License-Identifier: Apache-2.0

package braga

import "encoding/json"

// Result is the code returned by every fallible braga_* entry point, mirroring braga_result.
type Result int32

// The frozen ABI result discriminants (braga.h 0..9).
const (
	ResultOK         Result = 0
	ResultInvalidArg Result = 1
	ResultConfig     Result = 2
	ResultKey        Result = 3
	ResultStore      Result = 4
	ResultEmbed      Result = 5
	ResultNotFound   Result = 6
	ResultIO         Result = 7
	ResultPanic      Result = 8
	ResultOOM        Result = 9
)

// String returns the header's constant name for the code.
func (r Result) String() string {
	switch r {
	case ResultOK:
		return "OK"
	case ResultInvalidArg:
		return "INVALID_ARG"
	case ResultConfig:
		return "CONFIG"
	case ResultKey:
		return "KEY"
	case ResultStore:
		return "STORE"
	case ResultEmbed:
		return "EMBED"
	case ResultNotFound:
		return "NOT_FOUND"
	case ResultIO:
		return "IO"
	case ResultPanic:
		return "PANIC"
	case ResultOOM:
		return "OOM"
	default:
		return "UNKNOWN"
	}
}

// Status is the reconciler state reported by Braga.Status, mirroring braga_status_code.
type Status int32

// The frozen ABI status discriminants (braga.h 0..3).
const (
	StatusUpToDate Status = 0
	StatusChecking Status = 1
	StatusUpdating Status = 2
	StatusError    Status = 3
)

// String returns the header's constant name for the status.
func (s Status) String() string {
	switch s {
	case StatusUpToDate:
		return "UP_TO_DATE"
	case StatusChecking:
		return "CHECKING"
	case StatusUpdating:
		return "UPDATING"
	case StatusError:
		return "ERROR"
	default:
		return "UNKNOWN"
	}
}

// statusFromCode decodes a raw status value and fails closed to StatusError for anything unknown,
// matching the library's own mapping of out-of-range atomic values.
func statusFromCode(value Status) Status {
	switch value {
	case StatusUpToDate, StatusChecking, StatusUpdating, StatusError:
		return value
	default:
		return StatusError
	}
}

// Hit is one retrieval hit, mirroring the JSON object braga_query emits.
type Hit struct {
	ChunkID   string  `json:"chunk_id"`
	Path      string  `json:"path"`
	LineStart int     `json:"line_start"`
	LineEnd   int     `json:"line_end"`
	Content   string  `json:"content"`
	Score     float64 `json:"score"`
}

// DoctorCheck is one check in the doctor report, in the order the library ran it.
type DoctorCheck struct {
	Name   string `json:"name"`
	Passed bool   `json:"passed"`
	Detail string `json:"detail"`
}

// DoctorReport is the claims verifier's outcome: the ordered checks and their conjunction.
type DoctorReport struct {
	Ok     bool          `json:"ok"`
	Checks []DoctorCheck `json:"checks"`
}

// parseHits decodes the query's JSON array.
func parseHits(data []byte) ([]Hit, error) {
	hits := []Hit{}
	if err := json.Unmarshal(data, &hits); err != nil {
		return nil, err
	}
	return hits, nil
}

// parseDoctor decodes the doctor report's JSON object.
func parseDoctor(data []byte) (DoctorReport, error) {
	var report DoctorReport
	err := json.Unmarshal(data, &report)
	return report, err
}
