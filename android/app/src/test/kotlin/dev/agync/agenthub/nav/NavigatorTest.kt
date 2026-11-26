package dev.agync.agenthub.nav

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NavigatorTest {
  @Test fun `back pops to the root and then yields to the system`() {
    val n = Navigator()
    n.push(Route.Item("a"))
    n.push(Route.Session("s"))
    assertTrue(n.pop())
    assertEquals(Route.Item("a"), n.current)
    assertTrue(n.pop())
    assertFalse(n.pop())
    assertEquals(Route.Now, n.current)
  }

  @Test fun `switching tabs starts a fresh stack`() {
    val n = Navigator()
    n.push(Route.Item("a"))
    n.selectTab(Route.Hub)
    assertEquals(listOf<Route>(Route.Hub), n.stack.value)
    assertEquals(Route.Hub, n.tab)
  }

  @Test fun `an ask opens the next one in its place`() {
    val n = Navigator()
    n.push(Route.Item("a"))
    n.replace(Route.Item("b"))
    assertEquals(listOf(Route.Now, Route.Item("b")), n.stack.value)
  }

  @Test fun `a notification opens its ask above Now`() {
    val n = Navigator(Route.Hub)
    n.openFromOutside(Route.Item("x"))
    assertEquals(listOf(Route.Now, Route.Item("x")), n.stack.value)
  }

  @Test fun `pushing the screen already showing does nothing`() {
    val n = Navigator()
    n.push(Route.Settings)
    n.push(Route.Settings)
    assertEquals(2, n.stack.value.size)
  }

  @Test fun `the bottom bar shows on tabs, projects, sessions and members only`() {
    assertTrue(Route.Now.showsNav)
    assertTrue(Route.Project("p").showsNav)
    assertTrue(Route.Session("s").showsNav)
    assertFalse(Route.Item("i").showsNav)
    assertFalse(Route.Viewer("v").showsNav)
    assertFalse(Route.Settings.showsNav)
  }
}
