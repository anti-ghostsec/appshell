package app.folio

import android.content.Intent
import android.net.Uri
import android.os.StrictMode
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.appshell.ShellActivity
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.TimeUnit

/** Folio does its job: opens a file it's handed, shows it, saves an edit back into it. */
@RunWith(AndroidJUnit4::class)
class FolioTest {
    private val inst = InstrumentationRegistry.getInstrumentation()
    private fun script(name: String) = listOf("common.js", name).joinToString("\n") {
        inst.context.assets.open("selftest/$it").bufferedReader().readText()
    }

    @Test
    fun opensEditsAndSaves() {
        StrictMode.setVmPolicy(StrictMode.VmPolicy.Builder().build())
        val ctx = inst.targetContext
        val dir = File(ctx.filesDir, "selftest").apply { deleteRecursively(); mkdirs() }
        val file = File(dir, "note.txt").apply { writeText("hello from the test\n") }
        ShellActivity.resetTest()
        ctx.startActivity(
            Intent(Intent.ACTION_VIEW, Uri.fromFile(file)).setClassName(ctx, "app.folio.FolioActivity")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(ShellActivity.EXTRA_TEST_SCRIPT, script("folio.js"))
        )
        assertTrue("no report from the page", ShellActivity.testLatch.await(180, TimeUnit.SECONDS))
        val report = JSONObject(ShellActivity.testReport!!)
        assertTrue(report.toString(), report.getBoolean("ok"))
        assertTrue("the edit isn't in the file", file.readText().contains("edited by the test"))
    }
}
