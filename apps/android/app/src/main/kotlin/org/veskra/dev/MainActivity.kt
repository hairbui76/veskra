package org.veskra.dev

import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import org.veskra.dev.ui.VeskraApp
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.theme.VeskraTheme
import org.veskra.dev.update.AppUpdater

/**
 * Single activity. Edge-to-edge; the engine session, display tiles and editor state live in
 * [VeskraViewModel] so rotation/resize/multi-window never drop the document (MASTER_PLAN §2.2).
 * System Back / predictive back is handled in Compose with BackHandler.
 *
 * singleTop (manifest): the system installer reports an in-app update's progress back here through
 * [onNewIntent] (ADR-020).
 */
class MainActivity : ComponentActivity() {
    private val vm: VeskraViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        lifecycle.addObserver(
            object : DefaultLifecycleObserver {
                // Settle in-flight strokes (input.reset) and write a recovery checkpoint. Runs on the
                // engine thread / IO via the ViewModel; nothing blocks the UI thread here.
                override fun onPause(owner: LifecycleOwner) = vm.onPause()

                // Back from "Install unknown apps" in Settings: the update may be installable now.
                override fun onResume(owner: LifecycleOwner) = vm.onResume()
            },
        )
        setContent {
            VeskraTheme {
                VeskraApp(vm)
            }
        }
        handleInstallStatus(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleInstallStatus(intent)
    }

    private fun handleInstallStatus(intent: Intent?) {
        if (intent == null || !AppUpdater.isInstallStatus(intent)) return
        val status = AppUpdater.status(intent)
        if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            // The installer's own "Do you want to update this app?" screen.
            AppUpdater.confirmationIntent(intent)?.let { confirm ->
                runCatching { startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
                    .onFailure { vm.onInstallStatus(PackageInstaller.STATUS_FAILURE, it.message) }
            }
        }
        vm.onInstallStatus(status, AppUpdater.statusMessage(intent))
        setIntent(Intent(this, MainActivity::class.java)) // a later recreation must not replay it
    }
}
