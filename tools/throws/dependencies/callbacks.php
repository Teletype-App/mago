<?php
namespace VendorProbe;
/**
 * @param callable(): void $callback
 * @param-immediately-invoked-callable $callback
 */
function invoke(callable $callback): void { $callback(); }
/**
 * @param callable(): void $callback
 * @param-later-invoked-callable $callback
 */
function defer(callable $callback): void {}
