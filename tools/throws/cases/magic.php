<?php
declare(strict_types=1);
namespace ThrowsProbe\magic;
class ServerException extends \Exception {}
class QueueBroker {
    /** @throws ServerException */
    public function push(): void { throw new ServerException(); }
}
/** @property-read QueueBroker $queue */
class BaseApplication {
    public function __get(string $name): object { return new QueueBroker(); }
}
/** @mixin BaseApplication */
class WebApplication extends BaseApplication {}
class BaseYii {
    /** @var BaseApplication|WebApplication */
    public static $app;
}
class Yii extends BaseYii {}
abstract class ModelDecorator {
    /** @return static */
    public static function decorate(object $model): self { return new static(); }
}
/** @method static static decorate(object $model) */
abstract class PersonDecorator extends ModelDecorator {}
class PersonNotifier extends PersonDecorator {
    public function notify(): void { Yii::$app->queue->push(); }
}
function caller(): void { PersonNotifier::decorate(new \stdClass())->notify(); }
