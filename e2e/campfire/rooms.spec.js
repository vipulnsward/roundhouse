import { test, expect } from '@playwright/test'
import { signIn } from './helpers.js'

for (const kind of ['opens', 'closeds']) {
  test(`create and rename a ${kind} room through the shared form`, async ({ page }) => {
    const errors = []
    page.on('pageerror', error => errors.push(String(error)))
    page.on('console', message => {
      if (message.type() === 'error') errors.push(message.text())
    })
    await signIn(page)
    const name = `Room route check ${kind} ${Date.now()}`
    await page.goto(`/rooms/${kind}/new`)
    const form = page.locator('form').filter({ has: page.locator('#room_name') })
    await expect(form).toHaveAttribute('action', `/rooms/${kind}`)
    await page.locator('#room_name').fill(name)
    await page.getByRole('button', { name: 'Save', exact: true }).click()
    await expect(page).toHaveURL(/\/rooms\/\d+$/)
    const id = new URL(page.url()).pathname.split('/').pop()

    await page.goto(`/rooms/${kind}/${id}/edit`)
    await expect(page.locator('#room_name')).toHaveValue(name)
    await expect(form).toHaveAttribute('action', `/rooms/${kind}/${id}`)
    const renamed = `${name} renamed`
    await page.locator('#room_name').fill(renamed)
    await page.getByRole('button', { name: 'Save', exact: true }).click()
    await expect(page).toHaveURL(new RegExp(`/rooms/${id}$`))
    await page.goto(`/rooms/${kind}/${id}/edit`)
    await expect(page.locator('#room_name')).toHaveValue(renamed)
    expect(errors).toEqual([])

    page.once('dialog', dialog => dialog.accept())
    await page.getByRole('button', { name: `Delete ${renamed}`, exact: true }).click()
    await expect(page).not.toHaveURL(new RegExp(`/rooms/${kind}/${id}/edit$`))
  })
}
