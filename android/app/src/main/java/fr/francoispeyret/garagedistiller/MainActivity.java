package fr.francoispeyret.garagedistiller;

import android.Manifest;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;
import android.widget.Toast;
import androidx.core.app.NotificationCompat;
import com.google.androidgamesdk.GameActivity;

public final class MainActivity extends GameActivity {
    static { System.loadLibrary("garage_distiller"); }
    private static final String CHANNEL = "offers";
    private static native void setResumed(boolean resumed);

    private static native void setKeyboardInset(int pixels);

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        android.view.View decor = getWindow().getDecorView();
        decor.getViewTreeObserver().addOnGlobalLayoutListener(() -> {
            android.graphics.Rect visible = new android.graphics.Rect();
            decor.getWindowVisibleDisplayFrame(visible);
            int hidden = decor.getHeight() - visible.bottom;
            if (Build.VERSION.SDK_INT >= 30 && decor.getRootWindowInsets() != null) {
                android.view.WindowInsets insets = decor.getRootWindowInsets();
                hidden = insets.getInsets(android.view.WindowInsets.Type.ime()).bottom
                    - insets.getInsets(android.view.WindowInsets.Type.navigationBars()).bottom;
            }
            setKeyboardInset(hidden > decor.getHeight() / 5 ? hidden : 0);
        });
        getSystemService(NotificationManager.class).createNotificationChannel(
            new NotificationChannel(CHANNEL, "Nouvelles offres", NotificationManager.IMPORTANCE_DEFAULT));
    }
    @Override protected void onResume() { super.onResume(); setResumed(true); }
    @Override protected void onPause() { setResumed(false); super.onPause(); }

    // Called from the Rust worker; all permission and notification UI uses the Java UI thread.
    public void notifyOffer(String body) {
        runOnUiThread(() -> {
            if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                requestPermissions(new String[] { Manifest.permission.POST_NOTIFICATIONS }, 1);
                Toast.makeText(this, "Autorisez les notifications puis relancez le test. Les offres restent dans Détections.", Toast.LENGTH_LONG).show();
                return;
            }
            Intent intent = new Intent(this, MainActivity.class);
            intent.setFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP);
            PendingIntent content = PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
            getSystemService(NotificationManager.class).notify(1,
                new NotificationCompat.Builder(this, CHANNEL)
                    .setSmallIcon(R.drawable.ic_notification)
                    .setContentTitle("Garage Distiller · Bonne affaire repérée")
                    .setContentText(body).setStyle(new NotificationCompat.BigTextStyle().bigText(body))
                    .setContentIntent(content).setAutoCancel(true).build());
        });
    }
}
