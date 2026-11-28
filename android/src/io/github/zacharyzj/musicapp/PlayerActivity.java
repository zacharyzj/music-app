package io.github.zacharyzj.musicapp;

import android.app.NativeActivity;
import android.content.ClipData;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.AtomicFile;
import android.util.Log;
import android.widget.Toast;

import java.io.*;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

public final class PlayerActivity extends NativeActivity {
    private static final int MUSIC = 21, PLUGIN = 22;
    private final ExecutorService imports = Executors.newSingleThreadExecutor();
    static { System.loadLibrary("music_app_ui"); }
    static native void event(String operation, String value);
    @Override protected void onCreate(Bundle state) {
        setVolumeControlStream(android.media.AudioManager.STREAM_MUSIC);
        super.onCreate(state);
    }

    public void pickFiles(boolean plugin) {
        runOnUiThread(() -> {
            Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE);
            picker.setType(plugin ? "*/*" : "audio/*");
            picker.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, !plugin);
            try { startActivityForResult(picker, plugin ? PLUGIN : MUSIC); }
            catch (android.content.ActivityNotFoundException e) { message("未找到系统文件选择器"); }
        });
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if ((request != MUSIC && request != PLUGIN) || result != RESULT_OK || data == null) return;
        LinkedHashSet<Uri> selected = new LinkedHashSet<>();
        ClipData clips = data.getClipData();
        if (clips != null) for (int i=0; i<clips.getItemCount(); i++) selected.add(clips.getItemAt(i).getUri());
        if (data.getData() != null) selected.add(data.getData());
        selected.remove(null);
        if (selected.isEmpty()) return;
        imports.execute(() -> {
            File directory = request == MUSIC ? new File(getFilesDir(), "music") : new File(getCacheDir(), "plugin-import");
            boolean success = false;
            ArrayList<String> failures = new ArrayList<>();
            for (Uri uri : selected) {
                try {
                    File file = copyDocument(uri, directory, request == PLUGIN);
                    if (request == PLUGIN) event("plugin", file.getAbsolutePath());
                    success = true;
                } catch (Exception e) { failures.add(e.getMessage()); }
            }
            if (success && request == MUSIC) event("music", directory.getAbsolutePath());
            if (!failures.isEmpty()) message("导入失败 · " + failures.get(0));
        });
    }

    private File copyDocument(Uri uri, File directory, boolean plugin) throws Exception {
        if (!"content".equals(uri.getScheme())) throw new IOException("请选择系统文档");
        String name = null;
        try (Cursor cursor = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) name = cursor.getString(0);
        }
        if (name == null || name.isEmpty()) throw new IOException("无法读取文件名");
        name = name.replace('/', '_').replace('\\', '_');
        if (name.equals(".") || name.equals("..")) throw new IOException("无效的文件名");
        byte[] digest = MessageDigest.getInstance("SHA-256").digest(uri.toString().getBytes(java.nio.charset.StandardCharsets.UTF_8));
        StringBuilder id = new StringBuilder();
        for (int i=0; i<12; i++) id.append(String.format(java.util.Locale.ROOT, "%02x", digest[i]));
        // Keep the original filename for untagged music. Different document providers may use the same name.
        File target = new File(directory, id.toString());
        Files.createDirectories(target.toPath());
        File file = new File(target, name);
        File temp = File.createTempFile("import-", ".part", target);
        try {
            try (InputStream in = getContentResolver().openInputStream(uri); FileOutputStream out = new FileOutputStream(temp)) {
                if (in == null) throw new IOException("无法打开 " + name);
                byte[] buf = new byte[64 * 1024];
                long total = 0;
                int n;
                while ((n=in.read(buf)) != -1) {
                    total += n;
                    if (plugin && total > 16 * 1024 * 1024) throw new IOException("插件包过大");
                    out.write(buf, 0, n);
                }
            }
            Files.move(temp.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
        } finally { temp.delete(); }
        return file;
    }

    public synchronized byte[] pluginStorageKey() throws Exception {
        String alias = "music-player-plugin-storage-v1";
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        SecretKey wrapping = (SecretKey)store.getKey(alias, null);
        if (wrapping == null) {
            KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
            generator.init(new KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build());
            wrapping = generator.generateKey();
        }
        AtomicFile file = new AtomicFile(new File(getNoBackupFilesDir(), "plugin-storage.key"));
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        if (file.getBaseFile().exists()) {
            byte[] payload = file.readFully();
            if (payload.length != 60) throw new IOException("无效的插件存储密钥");
            cipher.init(Cipher.DECRYPT_MODE, wrapping, new GCMParameterSpec(128, payload, 0, 12));
            return cipher.doFinal(payload, 12, payload.length - 12);
        }
        byte[] key = new byte[32];
        new SecureRandom().nextBytes(key);
        cipher.init(Cipher.ENCRYPT_MODE, wrapping);
        byte[] sealed = cipher.doFinal(key);
        FileOutputStream out = file.startWrite();
        try { out.write(cipher.getIV()); out.write(sealed); file.finishWrite(out); }
        catch (IOException e) { file.failWrite(out); throw e; }
        return key;
    }

    // Called by the Slint thread after the real player state changes.
    public void playback(boolean playing, String title, String artist, long position, long duration) {
        runOnUiThread(() -> PlaybackService.update(this, playing, title, artist, position, duration));
    }
    private void message(String value) { runOnUiThread(() -> Toast.makeText(this, value, Toast.LENGTH_LONG).show()); }
    @Override protected void onDestroy() {
        imports.shutdown();
        stopService(new Intent(this, PlaybackService.class));
        super.onDestroy();
    }
}
