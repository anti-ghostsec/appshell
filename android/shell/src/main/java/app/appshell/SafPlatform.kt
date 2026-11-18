package app.appshell

import android.content.Context
import android.content.res.Configuration
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import java.io.File
import java.io.FileNotFoundException

/**
 * Files for appshell-core: content:// URIs from the system's file picker, or
 * from another app's "Open with" or "Share". No storage permission, no
 * persisted grants: once the app is closed, it can't reach any of them again.
 * Settings are the one file the app writes on its own.
 */
class SafPlatform(private val context: Context, private val app: String) {
    private val resolver get() = context.contentResolver
    @Volatile private var error = ""

    private fun uri(h: String): Uri = Uri.parse(h)

    /** Called from Rust when read() returned null. */
    fun lastError(@Suppress("UNUSED_PARAMETER") unused: String): String = error

    fun read(h: String, limit: Long): ByteArray? {
        error = ""
        return try {
            val size = sizeOf(h)
            if (size != null && size > limit) {
                error = "TOO_LARGE"
                return null
            }
            resolver.openInputStream(uri(h)).use { input ->
                if (input == null) {
                    error = "NOT_FOUND"
                    return null
                }
                val out = java.io.ByteArrayOutputStream()
                val buf = ByteArray(1 shl 16)
                var total = 0L
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    total += n
                    if (total > limit) {
                        error = "TOO_LARGE"
                        return null
                    }
                    out.write(buf, 0, n)
                }
                out.toByteArray()
            }
        } catch (e: FileNotFoundException) {
            error = "NOT_FOUND"
            null
        } catch (e: SecurityException) {
            error = "this app isn't allowed to read it any more"
            null
        } catch (e: Exception) {
            error = e.message ?: e.javaClass.simpleName
            null
        }
    }

    /** null when written; otherwise what went wrong. */
    fun write(h: String, data: ByteArray): String? {
        return try {
            val out = try {
                resolver.openOutputStream(uri(h), "wt")
            } catch (e: Exception) {
                resolver.openOutputStream(uri(h), "w")
            } ?: return "the file can't be opened for writing"
            out.use {
                it.write(data)
                it.flush()
                if (it is java.io.FileOutputStream) it.fd.sync()
            }
            null
        } catch (e: SecurityException) {
            "this app may only read that file; use Save as"
        } catch (e: Exception) {
            e.message ?: e.javaClass.simpleName
        }
    }

    private fun sizeOf(h: String): Long? = query(h, OpenableColumns.SIZE) { c, i -> if (c.isNull(i)) null else c.getLong(i) }

    private fun <T> query(h: String, column: String, get: (android.database.Cursor, Int) -> T?): T? {
        val u = uri(h)
        if (u.scheme == "file") {
            val f = File(u.path ?: return null)
            @Suppress("UNCHECKED_CAST")
            return when (column) {
                OpenableColumns.SIZE -> if (f.exists()) f.length() as T else null
                OpenableColumns.DISPLAY_NAME -> f.name as T
                DocumentsContract.Document.COLUMN_LAST_MODIFIED -> if (f.exists()) f.lastModified() as T else null
                else -> null
            }
        }
        return try {
            resolver.query(u, arrayOf(column), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    val i = c.getColumnIndex(column)
                    if (i >= 0) get(c, i) else null
                } else null
            }
        } catch (e: Exception) {
            null
        }
    }

    fun mtime(h: String): Long = query(h, DocumentsContract.Document.COLUMN_LAST_MODIFIED) { c, i -> if (c.isNull(i)) null else c.getLong(i) } ?: -1L

    fun exists(h: String): Boolean = query(h, OpenableColumns.DISPLAY_NAME) { c, i -> c.getString(i) } != null

    fun name(h: String): String =
        query(h, OpenableColumns.DISPLAY_NAME) { c, i -> c.getString(i) } ?: (uri(h).lastPathSegment ?: "document").substringAfterLast('/')

    /** Where it is, when the provider says: "Documents" for primary:Documents/notes.txt. */
    fun folder(h: String): String {
        val u = uri(h)
        if (u.scheme == "file") return File(u.path ?: "").parent ?: ""
        return try {
            if (DocumentsContract.isDocumentUri(context, u) && u.authority == "com.android.externalstorage.documents") {
                DocumentsContract.getDocumentId(u).substringAfter(':', "").substringBeforeLast('/', "")
            } else ""
        } catch (e: Exception) {
            ""
        }
    }

    private val settingsFile get() = File(context.filesDir, "settings.json")

    fun loadSettings(): ByteArray? = try {
        if (settingsFile.exists()) settingsFile.readBytes() else null
    } catch (e: Exception) {
        null
    }

    fun storeSettings(data: ByteArray) {
        try {
            val tmp = File(context.filesDir, "settings.json.tmp")
            java.io.FileOutputStream(tmp).use {
                it.write(data)
                it.fd.sync()
            }
            tmp.renameTo(settingsFile)
        } catch (e: Exception) {
        }
    }

    fun systemDark(): Boolean =
        (context.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
}
