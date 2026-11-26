package dev.agync.agenthub.ui

import androidx.compose.ui.text.font.FontWeight
import dev.agync.agenthub.ui.screens.marked
import org.junit.Assert.assertEquals
import org.junit.Test

class MarkedTest {
  private fun boldParts(text: String, words: List<String>): List<String> {
    val a = marked(text, words)
    return a.spanStyles.filter { it.item.fontWeight == FontWeight.Bold }.map { a.text.substring(it.start, it.end) }
  }

  @Test fun `each matched word is bold, whatever its case`() {
    assertEquals(listOf("Dark", "map"), boldParts("Try a Dark map style", listOf("dark", "MAP")))
  }

  @Test fun `a word with regex characters matches as text`() {
    assertEquals(listOf("c++"), boldParts("learn c++ and c", listOf("c++")))
  }

  @Test fun `no words leaves the text plain`() {
    assertEquals(emptyList<String>(), boldParts("anything", emptyList()))
  }
}
