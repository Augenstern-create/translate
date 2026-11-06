package ai.ridge.translator;

/**
 * JNI 契约 v1 的 Java 侧声明（docs/39 §1.3, ADR-061）。
 *
 * 只有四个 native 方法；能力级调用统一走 {@link #nativeCall(String, String, String)}，
 * 这样新增能力不需要改 ABI。
 */
public final class NativeBridge {
    static {
        System.loadLibrary("translator_jni");
    }

    private NativeBridge() {}

    /** 契约版本；与 Rust 侧 PROTOCOL_VERSION 必须一致。 */
    public static native int nativeProtocolVersion();

    /** configJson → {"ok":true,"data":{"handle":"g:i"}} 或错误信封。 */
    public static native String nativeCreate(String configJson);

    /** handle + method + payloadJson → 结果信封。 */
    public static native String nativeCall(String handle, String method, String payloadJson);

    /** 幂等关闭。 */
    public static native String nativeClose(String handle);
}
