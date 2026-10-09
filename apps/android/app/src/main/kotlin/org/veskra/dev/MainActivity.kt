package org.veskra.dev

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

/**
 * Single activity. Edge-to-edge; the engine session, display tiles and editor state live in
 * [VeskraViewModel] so rotation/resize/multi-window never drop the document (MASTER_PLAN §2.2).
 * System Back / predictive back is handled in Compose with BackHandler.
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
            },
        )
        setContent {
            VeskraTheme {
                VeskraApp(vm)
            }
        }
    }
}
