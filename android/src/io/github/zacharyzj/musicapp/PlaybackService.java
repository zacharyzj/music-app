package io.github.zacharyzj.musicapp;

import android.app.*;
import android.content.*;
import android.media.*;
import android.media.session.*;
import android.os.IBinder;
import android.os.SystemClock;
import android.util.Log;

public final class PlaybackService extends Service {
    private static final String CHANNEL = "playback";
    private static PlaybackService active;
    private static boolean starting;
    private static boolean playing;
    private static String title = "", artist = "";
    private static long position, duration;
    private MediaSession session;
    private AudioManager audio;
    private AudioFocusRequest focus;
    private boolean focused;

    static void update(Context context, boolean state, String song, String by, long pos, long length) {
        playing = state; title = song; artist = by; position = pos; duration = length;
        if (active != null) active.publish();
        else if (state && !starting) {
            starting = true;
            try { context.startForegroundService(new Intent(context, PlaybackService.class)); }
            catch (RuntimeException e) { starting=false; Log.w("MusicPlayer","Background playback unavailable",e); }
        }
    }
    @Override public void onCreate() {
        super.onCreate();
        active=this; starting=false;
        getSystemService(NotificationManager.class).createNotificationChannel(
            new NotificationChannel(CHANNEL, "音乐播放", NotificationManager.IMPORTANCE_LOW));
        session = new MediaSession(this, "Music Player");
        session.setCallback(new MediaSession.Callback() {
            @Override public void onPlay() { PlayerActivity.event("play", ""); }
            @Override public void onPause() { PlayerActivity.event("pause", ""); }
            @Override public void onSkipToNext() { PlayerActivity.event("next", ""); }
            @Override public void onSkipToPrevious() { PlayerActivity.event("previous", ""); }
            @Override public void onSeekTo(long ms) { PlayerActivity.event("seek", Long.toString(ms)); }
            @Override public void onStop() { PlayerActivity.event("pause", ""); stopSelf(); }
        });
        audio=getSystemService(AudioManager.class);
        focus=new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(new AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC).build())
            .setOnAudioFocusChangeListener(change -> {
                if (change < 0 && playing) PlayerActivity.event("pause", "");
            }).build();
    }
    @Override public int onStartCommand(Intent intent, int flags, int id) {
        publish();
        return START_NOT_STICKY;
    }
    private void publish() {
        session.setActive(true);
        session.setMetadata(new MediaMetadata.Builder().putString(MediaMetadata.METADATA_KEY_TITLE,title)
            .putString(MediaMetadata.METADATA_KEY_ARTIST,artist).putLong(MediaMetadata.METADATA_KEY_DURATION,duration).build());
        session.setPlaybackState(new PlaybackState.Builder().setActions(PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE
            | PlaybackState.ACTION_PLAY_PAUSE | PlaybackState.ACTION_SKIP_TO_NEXT | PlaybackState.ACTION_SKIP_TO_PREVIOUS
            | PlaybackState.ACTION_SEEK_TO | PlaybackState.ACTION_STOP)
            .setState(playing ? PlaybackState.STATE_PLAYING : PlaybackState.STATE_PAUSED, position, playing ? 1f : 0f,
                SystemClock.elapsedRealtime()).build());
        PendingIntent open = PendingIntent.getActivity(this, 0, new Intent(this, PlayerActivity.class),
            PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        session.setSessionActivity(open);
        Notification notification = new Notification.Builder(this,CHANNEL).setSmallIcon(android.R.drawable.ic_media_play)
            .setContentTitle(title).setContentText(artist).setContentIntent(open).setOnlyAlertOnce(true)
            .setStyle(new Notification.MediaStyle().setMediaSession(session.getSessionToken())).build();
        startForeground(1,notification);
        if (playing && !focused) {
            focused=audio.requestAudioFocus(focus) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED;
            if (!focused) PlayerActivity.event("pause", "");
        } else if (!playing) {
            if (focused) { audio.abandonAudioFocusRequest(focus); focused=false; }
            stopForeground(STOP_FOREGROUND_DETACH);
        }
    }
    @Override public IBinder onBind(Intent intent) { return null; }
    @Override public void onDestroy() {
        if (focused) audio.abandonAudioFocusRequest(focus);
        session.release();
        stopForeground(STOP_FOREGROUND_REMOVE);
        active=null; starting=false; playing=false;
        super.onDestroy();
    }
}
