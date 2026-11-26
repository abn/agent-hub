package dev.agync.agenthub.ui

import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsNodeInteractionCollection
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.junit4.ComposeTestRule
import androidx.compose.ui.unit.dp
import org.junit.Assert.fail

/** Fails when a clickable thing has no name, or its touch target is under 48 by 48. A control drawn at 40 gets a 48 target around it. */
fun ComposeTestRule.assertEveryControlIsNamedAndBigEnough() {
  val nodes: SemanticsNodeInteractionCollection = onAllNodes(hasClickAction())
  val density = density()
  val problems = mutableListOf<String>()
  for (n in nodes.fetchSemanticsNodes()) {
    val text = n.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text }.orEmpty()
    val desc = n.config.getOrNull(SemanticsProperties.ContentDescription)?.joinToString().orEmpty()
    val label = (text + desc).ifBlank { n.config.getOrNull(SemanticsProperties.EditableText)?.text.orEmpty() }
    val box = n.touchBoundsInRoot
    val w = with(density) { box.width.toDp() }
    val h = with(density) { box.height.toDp() }
    if (label.isBlank()) problems += "unnamed control at ${n.boundsInRoot}"
    if (w < 48.dp || h < 48.dp) problems += "'$label' is ${w.value} by ${h.value}dp"
  }
  if (problems.isNotEmpty()) fail(problems.joinToString("\n"))
}

private fun ComposeTestRule.density() = androidx.compose.ui.unit.Density(org.robolectric.RuntimeEnvironment.getApplication().resources.displayMetrics.density)
