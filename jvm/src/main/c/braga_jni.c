/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * JNI shim over the frozen bRAGa C ABI (version 1).
 *
 * Java strings cross as byte[] and are passed with an explicit length: JNI's GetStringUTFChars
 * produces modified UTF-8, which is not the valid UTF-8 braga.h requires. Only braga_last_error is
 * returned as a String, because the header guarantees sanitized ASCII there.
 *
 * The shim owns every braga_* call: on a non-OK result it reads the thread-local error on the
 * calling thread, throws dev.braga.BragaException(int, String), and returns a null/zero value.
 */

#include <jni.h>
#include <stddef.h>
#include <stdint.h>

#include "braga.h"

/* Renders the opaque handle as the jlong the Java layer carries. NULL is 0. */
static braga_handle *as_handle(jlong value) {
    return (braga_handle *)(intptr_t)value;
}

/*
 * Throws BragaException(code, sanitized message). The message is read here, on the calling
 * thread and before any other FFI call, because braga_last_error is valid only until then.
 */
static void throw_braga(JNIEnv *env, jint code, braga_handle *handle) {
    const char *message = braga_last_error(handle);
    if (message == NULL) {
        message = "";
    }
    jclass type = (*env)->FindClass(env, "dev/braga/BragaException");
    if (type == NULL) {
        return;
    }
    jmethodID constructor = (*env)->GetMethodID(env, type, "<init>", "(ILjava/lang/String;)V");
    if (constructor == NULL) {
        return;
    }
    jstring text = (*env)->NewStringUTF(env, message);
    if (text == NULL) {
        return;
    }
    jobject exception = (*env)->NewObject(env, type, constructor, code, text);
    if (exception != NULL) {
        (*env)->Throw(env, (jthrowable)exception);
    }
}

/* Pins a Java byte[] as a borrowed span. A null array stays NULL/0; a null pointer with a
 * non-zero length means an exception is already pending. */
static jbyte *acquire(JNIEnv *env, jbyteArray array, size_t *length) {
    *length = 0;
    if (array == NULL) {
        return NULL;
    }
    jsize size = (*env)->GetArrayLength(env, array);
    *length = (size_t)size;
    if (size == 0) {
        return NULL;
    }
    return (*env)->GetByteArrayElements(env, array, NULL);
}

/* Releases a pinned array. Both arguments are nullable so empty inputs are a no-op. */
static void release(JNIEnv *env, jbyteArray array, jbyte *bytes) {
    if (array != NULL && bytes != NULL) {
        (*env)->ReleaseByteArrayElements(env, array, bytes, JNI_ABORT);
    }
}

/* Copies a library-owned JSON buffer into a new jbyteArray, then frees the buffer. */
static jbyteArray json_bytes(JNIEnv *env, char *json, size_t length) {
    if (length > (size_t)INT32_MAX) {
        braga_string_free(json);
        throw_braga(env, BRAGA_OOM, NULL);
        return NULL;
    }
    jbyteArray out = (*env)->NewByteArray(env, (jsize)length);
    if (out != NULL && length > 0) {
        (*env)->SetByteArrayRegion(env, out, 0, (jsize)length, (const jbyte *)json);
    }
    braga_string_free(json);
    return out;
}

JNIEXPORT jint JNICALL Java_dev_braga_Native_abiVersion(JNIEnv *env, jclass type) {
    (void)env;
    (void)type;
    return (jint)braga_abi_version();
}

JNIEXPORT jlong JNICALL Java_dev_braga_Native_open(JNIEnv *env, jclass type, jbyteArray config_path,
                                                   jbyteArray key_path) {
    (void)type;
    size_t config_length = 0;
    size_t key_length = 0;
    jbyte *config = acquire(env, config_path, &config_length);
    jbyte *key = acquire(env, key_path, &key_length);
    if ((*env)->ExceptionCheck(env)) {
        release(env, config_path, config);
        release(env, key_path, key);
        return 0;
    }

    braga_handle *handle = NULL;
    braga_result code = braga_open((const uint8_t *)config, config_length,
                                   (const uint8_t *)key, key_length, &handle);
    release(env, config_path, config);
    release(env, key_path, key);
    if (code != BRAGA_OK) {
        throw_braga(env, (jint)code, NULL);
        return 0;
    }
    return (jlong)(intptr_t)handle;
}

JNIEXPORT jint JNICALL Java_dev_braga_Native_close(JNIEnv *env, jclass type, jlong handle) {
    (void)type;
    braga_result code = braga_close(as_handle(handle));
    if (code != BRAGA_OK) {
        throw_braga(env, (jint)code, as_handle(handle));
    }
    return (jint)code;
}

JNIEXPORT jint JNICALL Java_dev_braga_Native_status(JNIEnv *env, jclass type, jlong handle) {
    (void)env;
    (void)type;
    return (jint)braga_status(as_handle(handle));
}

JNIEXPORT jstring JNICALL Java_dev_braga_Native_lastError(JNIEnv *env, jclass type, jlong handle) {
    (void)type;
    const char *message = braga_last_error(as_handle(handle));
    return (*env)->NewStringUTF(env, message == NULL ? "" : message);
}

JNIEXPORT jint JNICALL Java_dev_braga_Native_update(JNIEnv *env, jclass type, jlong handle) {
    (void)type;
    /* The reserved key slot stays NULL in base mode; braga-extra makes it live without an ABI break. */
    braga_result code = braga_update(as_handle(handle), NULL, 0);
    if (code != BRAGA_OK) {
        throw_braga(env, (jint)code, as_handle(handle));
    }
    return (jint)code;
}

JNIEXPORT jbyteArray JNICALL Java_dev_braga_Native_query(JNIEnv *env, jclass type, jlong handle,
                                                        jbyteArray text, jint top_k) {
    (void)type;
    size_t text_length = 0;
    jbyte *input = acquire(env, text, &text_length);
    if ((*env)->ExceptionCheck(env)) {
        release(env, text, input);
        return NULL;
    }

    char *json = NULL;
    size_t json_length = 0;
    braga_result code = braga_query(as_handle(handle), (const uint8_t *)input, text_length,
                                    (uint32_t)top_k, NULL, 0, &json, &json_length);
    release(env, text, input);
    if (code != BRAGA_OK) {
        throw_braga(env, (jint)code, as_handle(handle));
        return NULL;
    }
    return json_bytes(env, json, json_length);
}

JNIEXPORT jbyteArray JNICALL Java_dev_braga_Native_doctor(JNIEnv *env, jclass type, jlong handle) {
    (void)type;
    char *json = NULL;
    size_t json_length = 0;
    braga_result code = braga_doctor(as_handle(handle), &json, &json_length);
    if (code != BRAGA_OK) {
        throw_braga(env, (jint)code, as_handle(handle));
        return NULL;
    }
    return json_bytes(env, json, json_length);
}

JNIEXPORT jint JNICALL Java_dev_braga_Native_init(JNIEnv *env, jclass type, jbyteArray config_path,
                                                  jbyteArray key_path) {
    (void)type;
    size_t config_length = 0;
    size_t key_length = 0;
    jbyte *config = acquire(env, config_path, &config_length);
    jbyte *key = acquire(env, key_path, &key_length);
    if ((*env)->ExceptionCheck(env)) {
        release(env, config_path, config);
        release(env, key_path, key);
        return BRAGA_OOM;
    }

    braga_result code = braga_init((const uint8_t *)config, config_length,
                                    (const uint8_t *)key, key_length);
    release(env, config_path, config);
    release(env, key_path, key);
    if (code != BRAGA_OK) {
        throw_braga(env, (jint)code, NULL);
    }
    return (jint)code;
}
