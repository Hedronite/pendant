// The D1 folder probe on Android: the Storage Access Framework tree picker
// (ACTION_OPEN_DOCUMENT_TREE), a persisted read permission on the tree, and
// a count of its .md files through DocumentsContract. A tree is a content://
// URI, never a POSIX path.
//
// Compiled to dex by build.rs with PendantKey.java and loaded the same way
// (platform/keystore.rs). The picker needs an activity result; nsh 0.5.0
// gives the app no Activity hook, so a headless Fragment (no manifest entry)
// is added to the app's resumed Activity to start the picker and take the
// result. Calls come from a worker thread, never the UI thread: pick()
// blocks until the picker ends.

package com.hedronite.pendant;

import android.app.Activity;
import android.app.Fragment;
import android.app.FragmentManager;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.content.UriPermission;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.DocumentsContract;
import java.lang.reflect.Field;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

public final class PendantFolders {
    /** The error of the last failed call. 0: none. */
    public static volatile int lastError;

    public static final int ERROR_CANCELLED = 1;
    public static final int ERROR_NO_ACTIVITY = -2;
    public static final int ERROR_INTERRUPTED = -3;
    public static final int ERROR_PROVIDER = -4;
    public static final int ERROR_NOT_KEPT = -5;

    private static final int REQUEST = 0x0D1;
    private static final String TAG = "pendant-d1-picker";

    private PendantFolders() {}

    /**
     * Shows the tree picker and returns the picked tree's URI, with its
     * read permission persisted, or null with lastError set.
     */
    public static String pick(int timeoutSeconds) {
        lastError = 0;
        final Activity activity = resumedActivity();
        if (activity == null) {
            lastError = ERROR_NO_ACTIVITY;
            return null;
        }
        final Request request = new Request();
        Picker.pending = request;
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                FragmentManager fragments = activity.getFragmentManager();
                fragments.beginTransaction().add(new Picker(), TAG).commitAllowingStateLoss();
            }
        });
        try {
            if (!request.done.await(timeoutSeconds, TimeUnit.SECONDS)) {
                lastError = ERROR_INTERRUPTED;
                return null;
            }
        } catch (InterruptedException e) {
            lastError = ERROR_INTERRUPTED;
            return null;
        }
        if (request.tree == null) {
            lastError = request.error != 0 ? request.error : ERROR_CANCELLED;
            return null;
        }
        try {
            activity.getContentResolver().takePersistableUriPermission(
                    request.tree, Intent.FLAG_GRANT_READ_URI_PERMISSION);
        } catch (SecurityException e) {
            lastError = ERROR_PROVIDER;
            return null;
        }
        return request.tree.toString();
    }

    /** Whether a read permission on tree is still persisted for the app. */
    public static boolean kept(String tree) {
        lastError = 0;
        Context context = application();
        if (context == null) {
            lastError = ERROR_NO_ACTIVITY;
            return false;
        }
        Uri uri = Uri.parse(tree);
        for (UriPermission permission : context.getContentResolver().getPersistedUriPermissions()) {
            if (permission.getUri().equals(uri) && permission.isReadPermission()) {
                return true;
            }
        }
        lastError = ERROR_NOT_KEPT;
        return false;
    }

    /**
     * Counts the tree: "mdTop,mdTotal,otherFiles,folders", or null with
     * lastError set. Hidden entries (a leading dot) are skipped.
     */
    public static String count(String tree) {
        lastError = 0;
        Context context = application();
        if (context == null) {
            lastError = ERROR_NO_ACTIVITY;
            return null;
        }
        try {
            Uri uri = Uri.parse(tree);
            int[] counts = new int[4];
            walk(context.getContentResolver(), uri, DocumentsContract.getTreeDocumentId(uri), true, counts);
            return counts[0] + "," + counts[1] + "," + counts[2] + "," + counts[3];
        } catch (Exception e) {
            lastError = ERROR_PROVIDER;
            return null;
        }
    }

    private static void walk(ContentResolver resolver, Uri tree, String parent, boolean top, int[] counts) {
        Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parent);
        String[] columns = {
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
        };
        Cursor cursor = resolver.query(children, columns, null, null, null);
        if (cursor == null) {
            return;
        }
        try {
            while (cursor.moveToNext()) {
                String id = cursor.getString(0);
                String name = cursor.getString(1);
                String mime = cursor.getString(2);
                if (name == null || name.startsWith(".")) {
                    continue;
                }
                if (DocumentsContract.Document.MIME_TYPE_DIR.equals(mime)) {
                    counts[3]++;
                    walk(resolver, tree, id, false, counts);
                } else if (name.toLowerCase(java.util.Locale.ROOT).endsWith(".md")) {
                    counts[1]++;
                    if (top) {
                        counts[0]++;
                    }
                } else {
                    counts[2]++;
                }
            }
        } finally {
            cursor.close();
        }
    }

    private static Context application() {
        try {
            return (Context) Class.forName("android.app.ActivityThread")
                    .getMethod("currentApplication")
                    .invoke(null);
        } catch (Exception e) {
            return null;
        }
    }

    /** The app's resumed Activity, from the framework's ActivityThread. */
    private static Activity resumedActivity() {
        try {
            Class<?> threadClass = Class.forName("android.app.ActivityThread");
            Object thread = threadClass.getMethod("currentActivityThread").invoke(null);
            Field activities = threadClass.getDeclaredField("mActivities");
            activities.setAccessible(true);
            for (Object record : ((Map<?, ?>) activities.get(thread)).values()) {
                Class<?> recordClass = record.getClass();
                Field paused = recordClass.getDeclaredField("paused");
                paused.setAccessible(true);
                if (!paused.getBoolean(record)) {
                    Field activity = recordClass.getDeclaredField("activity");
                    activity.setAccessible(true);
                    return (Activity) activity.get(record);
                }
            }
        } catch (Exception e) {
            return null;
        }
        return null;
    }

    private static final class Request {
        final CountDownLatch done = new CountDownLatch(1);
        volatile Uri tree;
        volatile int error;
    }

    /** Starts the picker and takes its result; removes itself after. */
    @SuppressWarnings("deprecation")
    public static final class Picker extends Fragment {
        static volatile Request pending;

        public Picker() {}

        @Override
        public void onCreate(Bundle state) {
            super.onCreate(state);
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                    | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
            try {
                startActivityForResult(intent, REQUEST);
            } catch (Exception e) {
                finish(null, ERROR_PROVIDER);
            }
        }

        @Override
        public void onActivityResult(int request, int result, Intent data) {
            if (request != REQUEST) {
                return;
            }
            Uri tree = (result == Activity.RESULT_OK && data != null) ? data.getData() : null;
            finish(tree, tree == null ? ERROR_CANCELLED : 0);
        }

        private void finish(Uri tree, int error) {
            Request current = pending;
            pending = null;
            if (current != null) {
                current.tree = tree;
                current.error = error;
                current.done.countDown();
            }
            if (getFragmentManager() != null) {
                getFragmentManager().beginTransaction().remove(this).commitAllowingStateLoss();
            }
        }
    }
}
