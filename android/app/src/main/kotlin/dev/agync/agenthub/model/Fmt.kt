package dev.agync.agenthub.model

import java.time.Duration
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

/** Time and size text. Times follow the phone's locale; ages are short words the board uses. */
class Fmt(private val zone: ZoneId = ZoneId.systemDefault(), private val locale: Locale = Locale.getDefault()) {
  private val timeFormat = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale).withZone(zone)
  private val dayFormat = DateTimeFormatter.ofPattern("EEE d MMM", locale).withZone(zone)
  private val dateFormat = DateTimeFormatter.ofPattern("d MMM", locale).withZone(zone)
  private val weekday = DateTimeFormatter.ofPattern("EEE", locale).withZone(zone)

  fun time(at: Instant): String = timeFormat.format(at)

  fun day(at: Instant): String = dayFormat.format(at)

  /** "14 min", "1 h", "Mon", "12 Nov": the age beside a row. */
  fun ago(at: Instant, now: Instant): String {
    val d = Duration.between(at, now)
    return when {
      d.isNegative || d.toMinutes() < 1 -> "now"
      d.toMinutes() < 60 -> "${d.toMinutes()} min"
      d.toHours() < 24 -> "${d.toHours()} h"
      d.toDays() < 7 -> weekday.format(at)
      else -> dateFormat.format(at)
    }
  }

  /** A time today, or the weekday, or the date: the clock column of a list. */
  fun stamp(at: Instant, now: Instant): String {
    val sameDay = at.atZone(zone).toLocalDate() == now.atZone(zone).toLocalDate()
    return when {
      sameDay -> time(at)
      Duration.between(at, now).toDays() < 7 -> weekday.format(at)
      else -> dateFormat.format(at)
    }
  }

  /** "in 3 h 28 min", for a deadline. */
  fun until(at: Instant, now: Instant): String {
    val d = Duration.between(now, at)
    if (d.isNegative) return "now"
    val h = d.toHours()
    val m = d.toMinutes() % 60
    return when {
      h > 0 && m > 0 -> "in $h h $m min"
      h > 0 -> "in $h h"
      else -> "in ${maxOf(m, 1)} min"
    }
  }

  fun date(at: Instant): String = dateFormat.format(at)

  companion object {
    fun bytes(n: Long): String =
      when {
        n >= 1_000_000_000L -> String.format(Locale.ROOT, "%.1f GB", n / 1e9)
        n >= 1_000_000L -> if (n >= 100_000_000L) "${n / 1_000_000} MB" else String.format(Locale.ROOT, "%.1f MB", n / 1e6)
        n >= 1_000L -> "${n / 1_000} KB"
        else -> "$n B"
      }
  }
}
