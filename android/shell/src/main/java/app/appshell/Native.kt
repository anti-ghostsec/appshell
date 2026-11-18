package app.appshell

/** appshell-core, in libappshell.so (android/ffi). */
object Native {
    init {
        System.loadLibrary("appshell")
    }

    external fun create(app: String, platform: SafPlatform, files: String): Long
    external fun destroy(id: Long)
    /** {"op": ...} in; a JSON list of actions (or a small object, for some ops) out. */
    external fun call(id: Long, request: String): String
    /** The bytes behind a token, after their MIME type and a newline; null if there are none. */
    external fun blobTake(id: Long, win: Int, token: String): ByteArray?
    external fun blobPut(id: Long, win: Int, data: ByteArray, kind: String): String?
    external fun exportDone(id: Long, req: Int, pdf: ByteArray?, error: String?): String
    external fun script(app: String, arrayBuffers: Boolean): String
}
