package app.leaflet

import android.content.Intent
import android.net.Uri
import android.os.StrictMode
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.appshell.ShellActivity
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.TimeUnit
import java.util.zip.ZipInputStream

/** Leaflet does its main jobs: documents edited and saved and exported, PDFs shown, changed and saved. */
@RunWith(AndroidJUnit4::class)
class LeafletTest {
    private val inst = InstrumentationRegistry.getInstrumentation()
    private fun asset(name: String) = inst.context.assets.open("selftest/$name").use { it.readBytes() }
    private fun script(name: String) = listOf("common.js", name).joinToString("\n") { String(asset(it)) }

    private fun run(fileName: String, test: String): Pair<File, File> {
        StrictMode.setVmPolicy(StrictMode.VmPolicy.Builder().build())
        val ctx = inst.targetContext
        val dir = File(ctx.filesDir, "selftest-$test").apply { deleteRecursively(); mkdirs() }
        val out = File(dir, "out").apply { mkdirs() }
        val file = File(dir, fileName).apply { writeBytes(asset(fileName)) }
        ShellActivity.resetTest()
        ctx.startActivity(
            Intent(Intent.ACTION_VIEW, Uri.fromFile(file)).setClassName(ctx, "app.leaflet.LeafletActivity")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NEW_DOCUMENT or Intent.FLAG_ACTIVITY_MULTIPLE_TASK)
                .putExtra(ShellActivity.EXTRA_TEST_SCRIPT, script("$test.js"))
                .putExtra(ShellActivity.EXTRA_TEST_SAVE, out.path)
                .putExtra(ShellActivity.EXTRA_TEST_SHOT, test)
        )
        assertTrue("no report from the page", ShellActivity.testLatch.await(240, TimeUnit.SECONDS))
        val report = JSONObject(ShellActivity.testReport!!)
        assertTrue(report.toString(), report.getBoolean("ok"))
        return file to out
    }

    private fun member(zip: ByteArray, name: String): String {
        ZipInputStream(zip.inputStream()).use { z ->
            while (true) {
                val e = z.nextEntry ?: return ""
                if (e.name == name) return String(z.readBytes())
            }
        }
    }

    @Test
    fun documentIsEditedSavedAndExported() {
        val (file, out) = run("sample.docx", "leaflet-doc")
        assertTrue("the edit isn't in the saved file", member(file.readBytes(), "word/document.xml").contains("Edited by the test."))
        val pdf = File(out, "sample.pdf")
        assertTrue("no exported PDF", pdf.exists())
        assertEquals("%PDF", String(pdf.readBytes().copyOfRange(0, 4)))
    }

    @Test
    fun pdfIsShownTurnedAndSavedAsANewFile() {
        val (file, out) = run("pages.pdf", "leaflet-pdf")
        val saved = File(out, "pages.pdf")
        assertTrue("nothing saved", saved.exists())
        assertEquals("%PDF", String(saved.readBytes().copyOfRange(0, 4)))
        assertFalse("the saved PDF is unchanged", saved.readBytes().contentEquals(file.readBytes()))
    }
}
