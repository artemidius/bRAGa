// SPDX-License-Identifier: Apache-2.0

package dev.braga;

/**
 * A failing {@code braga_*} call.
 *
 * <p>{@link #code()} is the ABI result discriminant and {@link #getMessage()} renders {@code
 * "CODE: message"}, or just {@code CODE} when the sanitized message is empty. The message never
 * contains key material, file content, or query text.
 */
public final class BragaException extends Exception {

    private static final long serialVersionUID = 1L;

    private final BragaResult code;

    /** Creates an exception from a typed result code and a sanitized message. */
    public BragaException(BragaResult code, String message) {
        super(render(code, message));
        this.code = code;
    }

    /** Creates an exception from the raw code the JNI shim throws. */
    public BragaException(int code, String message) {
        this(BragaResult.fromCode(code), message);
    }

    /** Returns the ABI result code. */
    public BragaResult code() {
        return code;
    }

    private static String render(BragaResult code, String message) {
        if (message == null || message.isEmpty()) {
            return code.name();
        }
        return code.name() + ": " + message;
    }
}
