// SPDX-License-Identifier: Apache-2.0

package dev.braga;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;

/**
 * The claims verifier's outcome: the ordered checks and their conjunction.
 *
 * @param ok conjunction of every check's {@code passed} flag
 * @param checks every check, in the fixed order the library ran it
 */
public record DoctorReport(
        @JsonProperty("ok") boolean ok,
        @JsonProperty("checks") List<DoctorCheck> checks) {}
