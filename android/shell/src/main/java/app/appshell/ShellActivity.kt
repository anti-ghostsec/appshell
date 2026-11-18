package app.appshell

import android.annotation.SuppressLint
import android.app.ActivityManager
import android.app.AlertDialog
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.print.PageRange
import android.print.PdfExport
import android.print.PrintAttributes
import android.print.PrintDocumentAdapter
import android.print.PrintDocumentInfo
import android.print.PrintManager
import android.util.Base64
import android.util.Log
import android.view.ViewGroup
import android.webkit.ConsoleMessage
import android.webkit.CookieManager
import android.webkit.GeolocationPermissions
import android.webkit.JavascriptInterface
import android.webkit.PermissionRequest
import android.webkit.RenderProcessGoneDetail
import android.webkit.SslErrorHandler
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebStorage
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebSettingsCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayInputStream
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.CountDownLatch

/**
 * One window of Folio or Leaflet: the app's interface in a WebView with
 * everything a browser would do on its own turned off. The page talks to
 * appshell-core (Native) through a message channel only it can use; this
 * class carries out what the core answers.
 */
abstract class ShellActivity : ComponentActivity() {
    /** "folio" or "leaflet". */
    abstract val app: String

    private lateinit var root: FrameLayout
    private var web: WebView? = null
    private var core = 0L
    @Volatile private var win = -1
    private var fullscreen = false
    private var pendingPut: Pair<Int, String>? = null
    private var dialogReq = -1
    private var dialogKind = ""
    private var startScript = ""
    private lateinit var picker: ActivityResultLauncher<Intent>

    private val debuggable get() = (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0
    /** Debug builds only: the instrumented tests drive the page through this. */
    private val test get() = if (debuggable) intent.getStringExtra(EXTRA_TEST_SCRIPT) else null

    companion object {
        const val HOST = "appassets.androidplatform.net"
        const val ORIGIN = "https://$HOST"
        const val INDEX = "$ORIGIN/index.html"
        const val EXTRA_WIN = "app.appshell.win"
        const val EXTRA_TEST_SCRIPT = "app.appshell.test.script"
        const val EXTRA_TEST_SAVE = "app.appshell.test.save"
        const val EXTRA_TEST_ASK = "app.appshell.test.ask"
        const val EXTRA_TEST_SHOT = "app.appshell.test.shot"
        private const val TAG = "appshell"

        @Volatile var testReport: String? = null
        @Volatile var testLatch = CountDownLatch(1)

        fun resetTest() {
            testReport = null
            testLatch = CountDownLatch(1)
        }

        private val MIME = mapOf(
            "html" to "text/html", "js" to "text/javascript", "mjs" to "text/javascript", "css" to "text/css",
            "svg" to "image/svg+xml", "png" to "image/png", "gif" to "image/gif", "json" to "application/json",
            "wasm" to "application/wasm", "ttf" to "font/ttf", "otf" to "font/otf", "woff" to "font/woff", "woff2" to "font/woff2",
        )

        fun mime(path: String) = MIME[path.substringAfterLast('.', "").lowercase()] ?: "application/octet-stream"

        /** The interface file a request names, relative to ui/, or null if it would leave ui/. */
        fun uiPath(path: String): String? {
            val parts = ArrayList<String>()
            for (p in path.split('/')) {
                when {
                    p.isEmpty() || p == "." -> {}
                    p == ".." || p.contains('\\') || p.contains(':') || p.contains('\u0000') -> return null
                    else -> parts.add(p)
                }
            }
            return if (parts.isEmpty()) null else parts.joinToString("/")
        }
    }

    private fun msg(vararg kv: Pair<String, Any?>): String {
        val o = JSONObject()
        for ((k, v) in kv) o.put(k, v ?: JSONObject.NULL)
        return o.toString()
    }

    // ------------------------------------------------------------ the window

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= 33) setRecentsScreenshotEnabled(false)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        root = FrameLayout(this)
        setContentView(root)
        ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.ime() or WindowInsetsCompat.Type.displayCutout()
            )
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            WindowInsetsCompat.CONSUMED
        }
        picker = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { r ->
            onPicked(r.resultCode, r.data)
        }
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() = requestClose()
        })

        val files = filesOf(intent)
        if (app == "folio") {
            Cores.drop() // each Folio window starts over: no tokens, no state from before
            core = Cores.get(this, app, files)
            perform(Native.call(core, msg("op" to "start")))
        } else {
            core = Cores.get(this, app, emptyList())
            val given = intent.getIntExtra(EXTRA_WIN, -1)
            if (given >= 0) {
                adopt(given)
            } else if (files.isEmpty()) {
                perform(Native.call(core, msg("op" to "activate")))
            } else {
                perform(Native.call(core, msg("op" to "start", "files" to JSONArray(files))))
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        val files = filesOf(intent)
        if (files.isNotEmpty()) perform(Native.call(core, msg("op" to "openFiles", "win" to win, "files" to JSONArray(files))))
    }

    /** What another app opened or shared with this one. */
    private fun filesOf(intent: Intent?): List<String> {
        intent ?: return emptyList()
        val out = ArrayList<String>()
        when (intent.action) {
            Intent.ACTION_VIEW, Intent.ACTION_EDIT -> intent.data?.let { out.add(it.toString()) }
            Intent.ACTION_SEND -> {
                @Suppress("DEPRECATION")
                (intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM))?.let { out.add(it.toString()) }
            }
            Intent.ACTION_SEND_MULTIPLE -> {
                @Suppress("DEPRECATION")
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)?.forEach { out.add(it.toString()) }
            }
        }
        return out.filter { it.startsWith("content:") || it.startsWith("file:") }
    }

    private fun adopt(w: Int) {
        win = w
        Cores.bind(w, this)
        makeWebView()
    }

    private fun requestClose() {
        if (win < 0) {
            finishAndRemoveTask()
            return
        }
        perform(Native.call(core, msg("op" to "closeRequested", "win" to win)))
    }

    private fun closeNow() {
        finishAndRemoveTask()
    }

    override fun onDestroy() {
        if (isFinishing && win >= 0) {
            Native.call(core, msg("op" to "closed", "win" to win))
            Cores.unbind(win)
            if (app == "folio") Cores.drop()
        }
        forget()
        super.onDestroy()
    }

    /** Nothing the page saw stays behind: caches, storage, cookies, history. */
    private fun forget() {
        web?.let {
            it.stopLoading()
            it.clearHistory()
            it.clearCache(true)
            it.clearFormData()
            root.removeView(it)
            it.destroy()
        }
        web = null
        WebStorage.getInstance().deleteAllData()
        CookieManager.getInstance().removeAllCookies(null)
        CookieManager.getInstance().flush()
        if (isFinishing) cacheDir.listFiles()?.forEach { it.deleteRecursively() }
    }

    // ------------------------------------------------------------ the WebView

    @SuppressLint("SetJavaScriptEnabled")
    private fun makeWebView() {
        if (debuggable) WebView.setWebContentsDebuggingEnabled(System.getProperty("appshell.debug") != null)
        val w = WebView(this)
        val s = w.settings
        s.javaScriptEnabled = true
        s.domStorageEnabled = false
        @Suppress("DEPRECATION")
        s.databaseEnabled = false
        s.allowFileAccess = false
        s.allowContentAccess = false
        @Suppress("DEPRECATION")
        s.allowFileAccessFromFileURLs = false
        @Suppress("DEPRECATION")
        s.allowUniversalAccessFromFileURLs = false
        s.setGeolocationEnabled(false)
        s.mediaPlaybackRequiresUserGesture = true
        s.cacheMode = WebSettings.LOAD_NO_CACHE
        s.javaScriptCanOpenWindowsAutomatically = false
        s.setSupportMultipleWindows(false)
        s.setSupportZoom(false)
        s.builtInZoomControls = false
        s.displayZoomControls = false
        s.userAgentString = app.replaceFirstChar { it.uppercase() }
        s.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
        if (WebViewFeature.isFeatureSupported(WebViewFeature.SAFE_BROWSING_ENABLE)) WebSettingsCompat.setSafeBrowsingEnabled(s, false)
        if (WebViewFeature.isFeatureSupported(WebViewFeature.REQUESTED_WITH_HEADER_ALLOW_LIST)) WebSettingsCompat.setRequestedWithHeaderOriginAllowList(s, emptySet())
        CookieManager.getInstance().setAcceptCookie(false)
        CookieManager.getInstance().setAcceptThirdPartyCookies(w, false)
        w.webViewClient = Client()
        w.webChromeClient = Chrome()
        w.setDownloadListener { _, _, _, _, _ -> }
        w.isHapticFeedbackEnabled = true

        val buffers = WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_ARRAY_BUFFER)
        if (WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
            WebViewCompat.addWebMessageListener(w, "__appshell", setOf(ORIGIN)) { _, message, origin, isMainFrame, _ ->
                if (!isMainFrame || origin.toString().trimEnd('/') != ORIGIN) return@addWebMessageListener
                if (buffers && message.type == WebMessageCompat.TYPE_ARRAY_BUFFER) onBuffer(message.arrayBuffer)
                else message.data?.let { onString(it) }
            }
        } else {
            // Older WebViews: the same channel as an object only this page reaches (nothing else ever loads).
            w.addJavascriptInterface(object {
                @JavascriptInterface
                fun postMessage(m: String) = runOnUiThread { onString(m) }
            }, "__appshell")
        }
        startScript = polyfills + "\n" + Native.script(app, buffers && WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) +
            "\n" + PHONE + "\n" + testScript()
        if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
            WebViewCompat.addDocumentStartJavaScript(w, startScript, setOf(ORIGIN))
            startScript = ""
        }
        web = w
        root.addView(w, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        val bg = JSONObject(Native.call(core, msg("op" to "background")))
        applyBackground(bg.optString("color", "#ffffff"), bg.optBoolean("dark"))
        w.loadUrl(INDEX)
    }

    private val polyfills by lazy { assets.open("appshell/polyfills.js").use { String(it.readBytes()) } }

    /**
     * The phone stylesheet (bigger touch targets, narrow-screen layouts:
     * assets/appshell), and for Leaflet: on a narrow screen the page list
     * starts closed, as it would cover the page. It's closed with the app's
     * own button, so the button goes on working as it does everywhere.
     */
    private val PHONE get() = """(function () {
  function add() {
    document.documentElement.classList.add('appshell-android');
    var l = document.createElement('link');
    l.rel = 'stylesheet';
    l.href = '/__appshell/phone.css';
    document.head.appendChild(l);
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', add); else add();
  if (${if (app == "leaflet") "true" else "false"}) {
    var closeListSoon = function () {
      var sb = document.getElementById('sidebar'), b = document.querySelector('[data-cmd="sidebar"]');
      if (!sb || !b || window.innerWidth >= 720) return;
      var done = false, obs;
      var check = function () { if (!done && !sb.hidden) { done = true; obs.disconnect(); b.click(); } };
      obs = new MutationObserver(check);
      obs.observe(sb, { attributes: true, attributeFilter: ['hidden'] });
      setTimeout(function () { obs.disconnect(); }, 15000);
    };
    var hook = setInterval(function () {
      var L = window.Leaflet;
      if (!L || typeof L.handle !== 'function') return;
      clearInterval(hook);
      var handle = L.handle.bind(L);
      L.handle = function (m) { if (m && m.what === 'load') closeListSoon(); return handle(m); };
    }, 20);
  }
})();"""

    private fun testScript(): String {
        val t = test ?: return ""
        return "window.__appshellReport = function (o) { __appshell.postMessage(JSON.stringify({ __test: o })); };\n$t"
    }

    private fun onString(s: String) {
        if (test != null && s.startsWith("{\"__test\"")) {
            val report = JSONObject(s).getJSONObject("__test").toString()
            screenshot(intent.getStringExtra(EXTRA_TEST_SHOT)) {
                testReport = report
                testLatch.countDown()
                closeNow()
            }
            return
        }
        if (s.startsWith("{\"put\":")) {
            val o = JSONObject(s)
            val id = o.getInt("put")
            val type = o.optString("type", "application/octet-stream")
            if (o.has("b64")) put(id, type, Base64.decode(o.getString("b64"), Base64.DEFAULT))
            else pendingPut = id to type
            return
        }
        perform(Native.call(core, msg("op" to "message", "win" to win, "msg" to s)))
    }

    /** Test builds: a picture of the window as the test left it, in files/screens/. */
    private fun screenshot(name: String?, then: () -> Unit) {
        if (name == null || root.width == 0 || root.height == 0) return then()
        val bmp = android.graphics.Bitmap.createBitmap(root.width, root.height, android.graphics.Bitmap.Config.ARGB_8888)
        val at = IntArray(2).also { root.getLocationInWindow(it) }
        val rect = android.graphics.Rect(at[0], at[1], at[0] + root.width, at[1] + root.height)
        android.view.PixelCopy.request(window, rect, bmp, { code ->
            if (code == android.view.PixelCopy.SUCCESS) {
                val dir = File(filesDir, "screens").apply { mkdirs() }
                FileOutputStream(File(dir, "$name.png")).use { bmp.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
            }
            then()
        }, android.os.Handler(mainLooper))
    }

    private fun onBuffer(data: ByteArray) {
        val (id, type) = pendingPut ?: return
        pendingPut = null
        put(id, type, data)
    }

    private fun put(id: Int, type: String, data: ByteArray) {
        val token = Native.blobPut(core, win, data, type)
        evalJs("__appshellPutDone($id, ${if (token == null) "null" else JSONObject.quote(token)})")
    }

    private fun response(kind: String, data: ByteArray, status: Int = 200): WebResourceResponse {
        val text = kind.startsWith("text/") || kind == "application/json" || kind == "image/svg+xml"
        val r = WebResourceResponse(kind, if (text) "utf-8" else null, ByteArrayInputStream(data))
        r.setStatusCodeAndReasonPhrase(status, if (status == 200) "OK" else "Not found")
        r.responseHeaders = mapOf("Cache-Control" to "no-store", "X-Content-Type-Options" to "nosniff")
        return r
    }

    private fun asset(path: String, kind: String): WebResourceResponse = try {
        response(kind, assets.open(path).use { it.readBytes() })
    } catch (e: Exception) {
        response("text/plain", ByteArray(0), 404)
    }

    /** Every request the page makes ends here: its own files, Leaflet's tokens, or nothing. */
    private fun serve(u: Uri): WebResourceResponse {
        if (u.scheme != "https" || u.host != HOST) return response("text/plain", ByteArray(0), 404)
        val path = u.path ?: "/"
        if (path.startsWith("/blob/")) {
            val b = Native.blobTake(core, win, path.removePrefix("/blob/")) ?: return response("text/plain", ByteArray(0), 404)
            val nl = b.indexOf('\n'.code.toByte())
            return response(String(b, 0, nl), b.copyOfRange(nl + 1, b.size))
        }
        if (path == "/__appshell/phone.css") return asset("appshell/phone-$app.css", "text/css")
        val rel = uiPath(path) ?: return response("text/plain", ByteArray(0), 404)
        if (rel.contains(".worker.") && (rel.endsWith(".mjs") || rel.endsWith(".js"))) {
            // PDF.js's worker gets the same fallbacks as the page (see assets/appshell/polyfills.js)
            return try {
                response(mime(rel), (polyfills + "\n").toByteArray() + assets.open("ui/$rel").use { it.readBytes() })
            } catch (e: Exception) {
                response("text/plain", ByteArray(0), 404)
            }
        }
        return asset("ui/$rel", mime(rel))
    }

    private inner class Client : WebViewClient() {
        override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse = serve(request.url)

        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
            val url = request.url.toString()
            if (url.substringBefore('#').substringBefore('?') == INDEX) return false
            perform(Native.call(core, msg("op" to "navigation", "win" to win, "url" to url, "user" to request.hasGesture())))
            return true
        }

        override fun onPageStarted(view: WebView, url: String?, favicon: android.graphics.Bitmap?) {
            if (startScript.isNotEmpty()) view.evaluateJavascript(startScript, null)
        }

        override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: android.net.http.SslError) = handler.cancel()

        override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
            if (view != web) return true
            root.removeView(view)
            view.destroy()
            web = null
            perform(Native.call(core, msg("op" to "crashed", "win" to win)))
            return true
        }
    }

    private inner class Chrome : WebChromeClient() {
        override fun onPermissionRequest(request: PermissionRequest) = request.deny()
        override fun onGeolocationPermissionsShowPrompt(origin: String?, callback: GeolocationPermissions.Callback) = callback.invoke(origin, false, false)
        override fun onCreateWindow(view: WebView?, isDialog: Boolean, isUserGesture: Boolean, resultMsg: android.os.Message?) = false
        override fun onConsoleMessage(m: ConsoleMessage): Boolean {
            if (debuggable) Log.d(TAG, "${m.messageLevel()} ${m.message()} (${m.sourceId()}:${m.lineNumber()})")
            return true
        }
    }

    // ------------------------------------------------------------ what the core asks for

    fun perform(json: String) {
        val list = try {
            JSONArray(json)
        } catch (e: Exception) {
            return
        }
        for (i in 0 until list.length()) {
            val a = list.optJSONObject(i) ?: continue
            try {
                performOne(a)
            } catch (e: Exception) {
                Log.w(TAG, "couldn't ${a.optString("do")}: $e")
            }
        }
    }

    private fun performOne(a: JSONObject) {
        val w = if (a.has("win")) a.getInt("win") else win
        val target = if (w == win) this else Cores.window(w)
        when (a.getString("do")) {
            "newWindow" -> newWindow(w)
            "openUrl" -> openUrl(a.getString("url"))
            else -> target?.performHere(a)
        }
    }

    private fun performHere(a: JSONObject) {
        when (a.getString("do")) {
            "eval" -> evalJs(a.getString("js"))
            "openDialog" -> openDialog(a)
            "saveDialog" -> saveDialog(a)
            "ask" -> ask(a)
            "title" -> setWindowTitle(a.getString("title"))
            "background" -> applyBackground(a.getString("color"), a.optBoolean("dark"))
            "fullscreen" -> toggleFullscreen()
            "close" -> closeNow()
            "present" -> {}
            "reload" -> if (web == null) makeWebView() else web?.loadUrl(INDEX)
            "printPage" -> printPage()
            "printPdf" -> printPdf(a.getString("token"), a.getString("name"))
            "exportPdf" -> exportPdf(a.getInt("req"))
        }
    }

    private fun newWindow(w: Int) {
        if (w == win) return
        if (win < 0) {
            adopt(w)
            return
        }
        startActivity(
            Intent(this, javaClass).putExtra(EXTRA_WIN, w)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT or Intent.FLAG_ACTIVITY_MULTIPLE_TASK)
        )
    }

    private fun evalJs(js: String) {
        val w = web ?: return
        if (js == "folio.requestClose()") {
            // If the page can't ask, it mustn't trap the user: close anyway.
            w.evaluateJavascript("(function () { try { folio.requestClose(); return 'ok'; } catch (e) { return 'fail'; } })()") { r ->
                if (r == null || !r.contains("ok")) perform(Native.call(core, msg("op" to "forceClose", "win" to win)))
            }
            return
        }
        w.evaluateJavascript(js, null)
    }

    private fun answer(req: Int, value: Any?) {
        perform(Native.call(core, msg("op" to "result", "req" to req, "value" to (value ?: JSONObject.NULL))))
    }

    private fun mimesOf(a: JSONObject): Array<String> {
        val filters = a.optJSONArray("filters") ?: return emptyArray()
        val first = filters.optJSONObject(0) ?: return emptyArray()
        val m = first.optJSONArray("mime") ?: return emptyArray()
        return Array(m.length()) { m.getString(it) }.filter { it != "*/*" }.toTypedArray()
    }

    private fun openDialog(a: JSONObject) {
        dialogReq = a.getInt("req")
        dialogKind = "open"
        val i = Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*")
        val mimes = mimesOf(a)
        if (mimes.isNotEmpty()) i.putExtra(Intent.EXTRA_MIME_TYPES, mimes)
        if (a.optBoolean("multiple")) i.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
        launch(i)
    }

    private fun saveDialog(a: JSONObject) {
        val r = a.getInt("req")
        val name = a.optString("name", "Untitled")
        if (test != null) {
            val dir = intent.getStringExtra(EXTRA_TEST_SAVE)
            answer(r, if (dir == null) null else Uri.fromFile(File(dir, name).also { it.parentFile?.mkdirs() }).toString())
            return
        }
        dialogReq = r
        dialogKind = "save"
        launch(
            Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                .setType(a.optString("mime", "application/octet-stream")).putExtra(Intent.EXTRA_TITLE, name)
        )
    }

    private fun launch(i: Intent) {
        try {
            picker.launch(i)
        } catch (e: ActivityNotFoundException) {
            Toast.makeText(this, "There's no app here for choosing files.", Toast.LENGTH_LONG).show()
            answer(dialogReq, null)
        }
    }

    private fun onPicked(code: Int, data: Intent?) {
        val r = dialogReq
        dialogReq = -1
        if (r < 0) return
        if (code != RESULT_OK || data == null) {
            answer(r, null)
            return
        }
        if (dialogKind == "open") {
            val list = JSONArray()
            val clip = data.clipData
            if (clip != null) for (k in 0 until clip.itemCount) list.put(clip.getItemAt(k).uri.toString())
            else data.data?.let { list.put(it.toString()) }
            answer(r, list)
        } else {
            answer(r, data.data?.toString())
        }
    }

    private fun ask(a: JSONObject) {
        val r = a.getInt("req")
        val labels = a.getJSONArray("buttons").let { b -> List(b.length()) { b.getString(it) } }
        val cancel = a.optInt("cancel", 0)
        if (test != null) {
            val want = intent.getStringExtra(EXTRA_TEST_ASK)
            answer(r, labels.indexOf(want).takeIf { it >= 0 } ?: cancel)
            return
        }
        val others = labels.indices.filter { it != cancel }
        val d = AlertDialog.Builder(this).setTitle(a.optString("message")).setMessage(a.optString("detail"))
        if (others.isNotEmpty()) d.setPositiveButton(labels[others.last()]) { _, _ -> answer(r, others.last()) }
        if (others.size > 1) d.setNeutralButton(labels[others.first()]) { _, _ -> answer(r, others.first()) }
        d.setNegativeButton(labels.getOrElse(cancel) { "Cancel" }) { _, _ -> answer(r, cancel) }
        d.setOnCancelListener { answer(r, cancel) }
        d.show()
    }

    /** Only links the user said yes to get here: appshell-core asks first. */
    private fun openUrl(url: String) {
        val l = url.lowercase()
        if (!(l.startsWith("http://") || l.startsWith("https://") || l.startsWith("mailto:"))) return
        try {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE))
        } catch (e: ActivityNotFoundException) {
            Toast.makeText(this, "There's no app here to open that link.", Toast.LENGTH_LONG).show()
        }
    }

    private fun setWindowTitle(t: String) {
        title = t
        @Suppress("DEPRECATION")
        setTaskDescription(ActivityManager.TaskDescription(t))
    }

    private fun applyBackground(color: String, dark: Boolean) {
        val c = try {
            Color.parseColor(color)
        } catch (e: Exception) {
            Color.WHITE
        }
        root.setBackgroundColor(c)
        window.decorView.setBackgroundColor(c)
        web?.setBackgroundColor(c)
        val ctl = WindowCompat.getInsetsController(window, root)
        ctl.isAppearanceLightStatusBars = !dark
        ctl.isAppearanceLightNavigationBars = !dark
    }

    private fun toggleFullscreen() {
        fullscreen = !fullscreen
        val ctl = WindowCompat.getInsetsController(window, root)
        if (fullscreen) {
            ctl.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            ctl.hide(WindowInsetsCompat.Type.systemBars())
        } else {
            ctl.show(WindowInsetsCompat.Type.systemBars())
        }
    }

    // ------------------------------------------------------------ printing

    private fun a4(): PrintAttributes = PrintAttributes.Builder()
        .setMediaSize(PrintAttributes.MediaSize.ISO_A4)
        .setResolution(PrintAttributes.Resolution("pdf", "pdf", 600, 600))
        // 22 mm at the sides, 20 mm top and bottom, in thousandths of an inch
        .setMinMargins(PrintAttributes.Margins(866, 787, 866, 787))
        .setColorMode(PrintAttributes.COLOR_MODE_COLOR)
        .build()

    private fun printPage() {
        val w = web ?: return
        val name = title?.toString()?.substringBefore(" — ") ?: app
        getSystemService(PrintManager::class.java).print(name, w.createPrintDocumentAdapter(name), a4())
    }

    private fun printPdf(token: String, name: String) {
        val b = Native.blobTake(core, win, token) ?: return
        val nl = b.indexOf('\n'.code.toByte())
        val data = b.copyOfRange(nl + 1, b.size)
        getSystemService(PrintManager::class.java).print(name, PdfBytes(name, data), null)
    }

    /** A PDF the page made, handed to the print service as it is. */
    private class PdfBytes(private val name: String, private val data: ByteArray) : PrintDocumentAdapter() {
        override fun onLayout(old: PrintAttributes?, new: PrintAttributes?, cancel: CancellationSignal?, cb: LayoutResultCallback, extras: Bundle?) {
            if (cancel?.isCanceled == true) {
                cb.onLayoutCancelled()
                return
            }
            cb.onLayoutFinished(PrintDocumentInfo.Builder(name).setContentType(PrintDocumentInfo.CONTENT_TYPE_DOCUMENT).build(), true)
        }

        override fun onWrite(pages: Array<out PageRange>?, dest: ParcelFileDescriptor, cancel: CancellationSignal?, cb: WriteResultCallback) {
            try {
                FileOutputStream(dest.fileDescriptor).use { it.write(data) }
                cb.onWriteFinished(arrayOf(PageRange.ALL_PAGES))
            } catch (e: Exception) {
                cb.onWriteFailed(e.message)
            }
        }
    }

    /**
     * Export as PDF: the page printed to a file in the app's own storage, read
     * back and removed; the page then strips the PDF's metadata and the core
     * writes it where the user chose.
     */
    private fun exportPdf(r: Int) {
        val w = web ?: return
        val dir = File(filesDir, "export").apply { mkdirs() }
        val file = File(dir, "${System.nanoTime()}.pdf")
        fun done(ok: Boolean, error: String?) = runOnUiThread {
            val bytes = if (ok && file.exists()) file.readBytes() else null
            file.delete()
            perform(Native.exportDone(core, r, bytes, error ?: if (bytes == null) "the page couldn't be printed" else null))
        }
        try {
            PdfExport.export(w.createPrintDocumentAdapter("export"), a4(), file) { ok, error -> done(ok, error) }
        } catch (e: Throwable) {
            // The print-to-file route isn't open on this Android: say so, and point to Print.
            done(false, "this phone doesn't let apps print to a file. Use Print and choose Save as PDF instead")
        }
    }
}
