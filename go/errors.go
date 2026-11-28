// SPDX-License-Identifier: Apache-2.0

package braga

// BragaError is a failing braga_* call.
//
// Code is the ABI result discriminant and Message is the sanitized text from braga_last_error: it
// never contains key material, file content, or query text.
type BragaError struct {
	Code    Result
	Message string
}

// Error renders the result code name followed by the sanitized message.
func (e *BragaError) Error() string {
	if e.Message == "" {
		return e.Code.String()
	}
	return e.Code.String() + ": " + e.Message
}
