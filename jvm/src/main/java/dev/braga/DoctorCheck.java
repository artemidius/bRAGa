// SPDX-License-Identifier: Apache-2.0

package dev.braga;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * One check in a {@link DoctorReport}, in the order the library ran it.
 *
 * @param name stable dotted key, for example {@code store.open}
 * @param passed whether the check passed
 * @param detail sanitized human-readable outcome
 */
public record DoctorCheck(
        @JsonProperty("name") String name,
        @JsonProperty("passed") boolean passed,
        @JsonProperty("detail") String detail) {}
