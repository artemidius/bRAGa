// SPDX-License-Identifier: Apache-2.0

package dev.braga;

/** The reconciler state reported by {@link Braga#status()}, mirroring {@code braga_status_code} in
 * {@code braga.h}. */
public enum BragaStatus {
    /** The index matches the scan directory; no update is in flight. */
    UP_TO_DATE(0),
    /** An update is in flight, nothing written yet. */
    CHECKING(1),
    /** An update is in flight and writing or embedding. */
    UPDATING(2),
    /** The last open or update failed. */
    ERROR(3);

    private final int code;

    BragaStatus(int code) {
        this.code = code;
    }

    /** Returns the frozen ABI discriminant. */
    public int code() {
        return code;
    }

    /** Decodes a raw status code; an unknown value fails closed to {@link #ERROR}. */
    public static BragaStatus fromCode(int value) {
        for (BragaStatus status : values()) {
            if (status.code == value) {
                return status;
            }
        }
        return ERROR;
    }
}
