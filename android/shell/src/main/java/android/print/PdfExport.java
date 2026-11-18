package android.print;

import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;

import java.io.File;

/**
 * Prints a WebView's page straight into a file, without the print dialog.
 *
 * Android has no public way to do this: the callbacks a print adapter
 * answers to can only be made from inside android.print, which is why this
 * class lives in that package. ShellActivity catches it failing (a future
 * Android could close the way) and sends the user to Print instead.
 */
public final class PdfExport {
    public interface Done {
        void done(boolean ok, String error);
    }

    private PdfExport() {}

    public static void export(final PrintDocumentAdapter adapter, final PrintAttributes attributes, final File file, final Done done) {
        adapter.onStart();
        adapter.onLayout(null, attributes, null, new PrintDocumentAdapter.LayoutResultCallback() {
            @Override
            public void onLayoutFinished(PrintDocumentInfo info, boolean changed) {
                final ParcelFileDescriptor out;
                try {
                    out = ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_CREATE
                            | ParcelFileDescriptor.MODE_TRUNCATE | ParcelFileDescriptor.MODE_READ_WRITE);
                } catch (Exception e) {
                    adapter.onFinish();
                    done.done(false, e.getMessage());
                    return;
                }
                adapter.onWrite(new PageRange[] {PageRange.ALL_PAGES}, out, new CancellationSignal(),
                        new PrintDocumentAdapter.WriteResultCallback() {
                            @Override
                            public void onWriteFinished(PageRange[] pages) {
                                close(out);
                                adapter.onFinish();
                                done.done(true, null);
                            }

                            @Override
                            public void onWriteFailed(CharSequence error) {
                                close(out);
                                adapter.onFinish();
                                done.done(false, error == null ? null : error.toString());
                            }

                            @Override
                            public void onWriteCancelled() {
                                close(out);
                                adapter.onFinish();
                                done.done(false, "cancelled");
                            }
                        });
            }

            @Override
            public void onLayoutFailed(CharSequence error) {
                adapter.onFinish();
                done.done(false, error == null ? null : error.toString());
            }

            @Override
            public void onLayoutCancelled() {
                adapter.onFinish();
                done.done(false, "cancelled");
            }
        }, null);
    }

    private static void close(ParcelFileDescriptor fd) {
        try {
            fd.close();
        } catch (Exception ignored) {
        }
    }
}
