package org.lotrax.scribblereader

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.util.Log
import android.view.WindowManager
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
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
                    showFolderContent(contentUri)
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

    val isGooglePlayGames: Boolean
        get() {
            val pm = packageManager
            return pm.hasSystemFeature("com.google.android.play.feature.HPE_EXPERIENCE")
        }

    private fun requestOpenFolderTree() {
        Log.d("main-activity", "Method called")
        val path = Environment.getExternalStorageDirectory()
        val uri = Uri.fromFile(path)
        launcher.launch(uri)
    }

    private fun showFolderContent(rootUri: Uri) {
        val cr = contentResolver
        try {
            cr.takePersistableUriPermission(rootUri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (e: SecurityException) {
            Log.e("main-activity", "Failed to get permission", e)
        }

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
                    Log.d("main-activity", "docId: $docId, name: $name, mime: $mime, size: $size")
                    if (isDirectory(mime)) {
                        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(rootUri, docId)
                        dirNodes.add(uri)
                    }
                }
            } finally {
                closeQuietly(c)
            }
        }

    }

    // Util method to check if the mime type is a directory
    private fun isDirectory(mimeType: String?): Boolean {
        return Document.MIME_TYPE_DIR == mimeType
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


    companion object {
        init {
            System.loadLibrary("main")
        }
    }
}
