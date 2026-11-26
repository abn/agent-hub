package dev.agync.agenthub.ui

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.HubState
import dev.agync.agenthub.model.NowBuilder
import dev.agync.agenthub.model.NowFilter
import dev.agync.agenthub.ui.screens.NowActions
import dev.agync.agenthub.ui.screens.NowScreen
import dev.agync.agenthub.ui.theme.HubTheme
import dev.agync.agenthub.viewmodel.NowScreenKind
import dev.agync.agenthub.viewmodel.NowUi
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** Until the hub has answered once, Now cannot say that nothing waits: it does not know. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xhdpi")
class LoadingTest {
  @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

  @Test fun `before the first answer Now says it is reaching the hub, not that nothing waits`() {
    val fmt = Fixtures.fmt()
    val ui = NowUi(Fixtures.hub.name, NowScreenKind.Loading, NowBuilder.build(HubState(Fixtures.hub), Fixtures.now, fmt), NowFilter.All, "", null, "Since you looked", Fixtures.now, emptyList(), emptyList(), null)
    compose.setContent { HubTheme { NowScreen(ui, fmt, NowActions({}, { _, _ -> }, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})) } }
    compose.onNodeWithText("Reaching ${Fixtures.hub.name}", substring = true).assertIsDisplayed()
    compose.onNodeWithText("Nothing waits on you.", substring = true).assertDoesNotExist()
  }
}
