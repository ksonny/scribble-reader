package org.lotrax.scribblereader

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.util.Log
import android.view.WindowManager
import android.view.KeyEvent
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.net.toUri
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.google.androidgamesdk.GameActivity
import java.io.Closeable
import java.util.LinkedList

class MainActivity : GameActivity() {
    private lateinit var launcher: ActivityResultLauncher<Uri?>

    private fun hideSystemUI() {
        // This will put the game behind any cutouts and waterfalls on devices which have
        // them, so the corresponding insets will be non-zero.
        window.attributes.layoutInDisplayCutoutMode =
            WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS

        // From API 30 onwards, this is the recommended way to hide the system UI, rather than
        // using View.setSystemUiVisibility.
        val decorView = window.decorView
        val controller = WindowInsetsControllerCompat(
            window,
            decorView
        )
        controller.hide(WindowInsetsCompat.Type.systemBars())
        controller.hide(WindowInsetsCompat.Type.displayCutout())
        controller.systemBarsBehavior =
            WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        launcher =
            registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { contentUri ->
                Log.i("main-activity", contentUri.toString())
                if (contentUri != null) {
                    try {
                        contentResolver.takePersistableUriPermission(contentUri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    } catch (e: SecurityException) {
                        Log.e("main-activity", "Failed to get permission", e)
                    }

                    wranglerOpenTree(contentUri.toString())
                }
            }

        // When true, the app will fit inside any system UI windows.
        // When false, we render behind any system UI windows.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        hideSystemUI()

        super.onCreate(savedInstanceState)
    }


    override fun onResume() {
        super.onResume()
        hideSystemUI()
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        // android-activity filters volume key events in activity glue, so we run a bypass
        if (keyCode == KeyEvent.KEYCODE_VOLUME_UP) {
            return inputKeyUp()
        } else if (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN) {
            return inputKeyDown()
        } else {
            return super.onKeyDown(keyCode, event)
        }
    }

    @Suppress("unused")
    val isGooglePlayGames: Boolean
        get() {
            val pm = packageManager
            return pm.hasSystemFeature("com.google.android.play.feature.HPE_EXPERIENCE")
        }

    @Suppress("unused")
    private fun discoverOpenTree() {
        val path = Environment.getExternalStorageDirectory()
        val uri = Uri.fromFile(path)
        launcher.launch(uri)
    }

    @Suppress("unused")
    private fun discoverFolderContent(ticketId: Long, rootUri: String) {
        val rootUri = rootUri.toUri()

        wranglerDiscoverStart(ticketId)

        val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(
            rootUri,
            DocumentsContract.getTreeDocumentId(rootUri)
        )

        val dirNodes: MutableList<Uri> = LinkedList<Uri>()
        dirNodes.add(childrenUri)

        while (!dirNodes.isEmpty()) {
            val childrenUri = dirNodes.removeAt(0)
            Log.d("main-activity", "node uri: $childrenUri")
            val c = contentResolver.query(
                childrenUri,
                arrayOf(
                    Document.COLUMN_DOCUMENT_ID,
                    Document.COLUMN_DISPLAY_NAME,
                    Document.COLUMN_MIME_TYPE,
                    Document.COLUMN_SIZE,
                    Document.COLUMN_LAST_MODIFIED,
                ),
                null,
                null,
                null
            )
            try {
                while (c!!.moveToNext()) {
                    val docId = c.getString(0)
                    val name = c.getString(1)
                    val mime = c.getString(2)
                    val size = c.getLong(3)
                    val lastModified = c.getLong(4)
                    if (Document.MIME_TYPE_DIR == mime) {
                        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(rootUri, docId)
                        dirNodes.add(uri)
                    } else {
                        wranglerDiscover(ticketId, docId, name, size, lastModified)
                    }
                }
            } catch (exception: Exception) {
                Log.e("main-activity", "Failed to list tree: $exception")
            } finally {
                closeQuietly(c)
            }
        }

        wranglerDiscoverEnd(ticketId)
    }

    @Suppress("unused")
    private fun openFileContent(ticketId: Long, rootUri: String, docId: String) {
        val rootUri = rootUri.toUri()
        val uri = DocumentsContract.buildDocumentUriUsingTree(rootUri, docId)

        try {
            val c = contentResolver.query(
                uri,
                arrayOf(
                    Document.COLUMN_SIZE,
                    Document.COLUMN_LAST_MODIFIED,
                ),
                null,
                null,
                null
            )
            if (c!!.moveToNext()) {
                val size = c.getLong(0)
                val lastModified = c.getLong(1)
                closeQuietly(c)

                val afd = contentResolver.openAssetFileDescriptor(uri, "r")
                if (afd != null) {
                    val pfd = afd.parcelFileDescriptor
                    val fd = pfd.fd

                    wranglerFile(ticketId, docId, fd, size, lastModified)

                    closeQuietly(pfd)
                } else {
                    Log.e("main-activity", "Failed to open file, got null result")
                    wranglerFail(ticketId, "Failed to open file, got null result")
                }
            }
        } catch (exception: Exception) {
            Log.e("main-activity", "Failed to open file: $exception")
            wranglerFail(ticketId, exception.toString())
        }
    }

    // Util method to close a closeable
    private fun closeQuietly(closeable: Closeable?) {
        if (closeable != null) {
            try {
                closeable.close()
            } catch (re: RuntimeException) {
                throw re
            } catch (_: Exception) {
                // ignore exception
            }
        }
    }

    external fun wranglerOpenTree(rootUri: String)
    external fun wranglerDiscoverStart(ticketId: Long)
    external fun wranglerDiscover(
        ticketId: Long,
        docId: String,
        name: String,
        size: Long,
        lastModified: Long
    )
    external fun wranglerDiscoverEnd(ticketId: Long)
    external fun wranglerFile(ticketId: Long, docId: String, fd: Int, size: Long, lastModified: Long)
    external fun wranglerFail(ticketId: Long, reason: String)

    external fun inputKeyUp(): Boolean
    external fun inputKeyDown(): Boolean

    companion object {
        init {
            System.loadLibrary("main")
        }
    }
}
